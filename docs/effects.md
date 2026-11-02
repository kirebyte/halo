# Effects

Effects run in the daemon as a layer of priority 100 (ambient): above the base color and below
OpenRGB and the games. If a game paints, the game shows; when it quits, the effect returns.

```bash
halo effect                    # which one is on and which exist
halo effect rainbow            # start one
halo effect comet fast         # effect and speed
halo effect slow               # speed only; restarts the running effect
halo effect off
halo effect colors tide 8000ff ff0080   # custom colors (`auto` restores the defaults)
halo effect direction comet reverse     # rainbow, cycle, comet and tide backwards
halo config                    # halo.conf: games, effect, speed, screen_off, latitude, longitude
halo config latitude 20.97     # change an option; applied at once when the daemon runs
```

| Effect | What it does | Period (slow / medium / fast) |
|---|---|---|
| `breathing` | each zone fades out and in with its base color (cosine squared); on the MCU unless 3+ colors are given | 5 / 3.5 / 2 s |
| `strobe` | the base colors, half the time on and half off | 1 / 0.5 / 0.4 s |
| `cycle` | the four zones go round the hue wheel together; on the MCU unless a palette is given | 12 / 8 / 4 s |
| `rainbow` | the hue wheel spread over the four zones, rotating; on the MCU unless a palette is given | 8 / 5 / 3 s |
| `comet` | a head in the color of left-bottom with a two-zone tail, running left-top → left-bottom → right-top → right-bottom | 2.5 / 1.6 / 1 s |
| `tide` | a deep-blue → aquamarine wave running through the four zones in the comet's order | 7 / 4.5 / 2.5 s |
| `starry` | a night-blue sky where stars are born at random (white, cool or warm), rise and fade | 2.2 / 1.6 / 1.1 s per star |
| `star` | one star at a time: a fast rise to pure white on a random zone (never the same twice in a row), with a halo on the ring's other LED | 2 / 1.4 / 0.9 s |
| `clock` | the time in 24 h, one LED per digit (below), breathing with the seconds | — |
| `clock12` | the same in 12 h (14:22 shows as 02:22) | — |
| `sun` | the color follows the height of the sun: purple at midnight, cool at night, warm by day, yellow at noon | — |
| `sensors` | left ring = chip temperature, right = CPU usage, from green through yellow to red | every 2 s |
| `battery` | the charge: red at 10 % or less, yellow half way, green when full; while charging a light flash walks the zones | every 10 s (0.75 s per zone while charging) |
| `pulse` | the four LEDs beat with the bass: they brighten towards white and the hue jumps 45° on every hit | — |
| `equalizer` | bars following the music (`parec` on the default output's monitor) | — |

- **Saved** in `~/.config/halo/halo.conf` (`effect`, `speed`) and restored when the daemon starts.
  The equalizer and the pulse are not saved, since they depend on the music.
- Colors come from the base color and are read every frame: `halo set ...` while an effect runs
  shows at once. The same goes for **custom colors** (`halo effect colors EFFECT C1 C2 ...`, hex or
  names), saved in `halo.conf` as `colors_EFFECT`; `auto` removes them. Meaning per effect:
  `breathing` 1 (that color to dark) or 2 (breathes between the two, never off); `strobe` and
  `comet` 1; `cycle` and `rainbow` 2 or more (a closed palette instead of the hue wheel); `tide`
  deep and crest; `starry` sky then stars; `star` sky and star; `pulse` any number (jumps to the
  next on each beat). Clock, sun, sensors and battery have none, because their color means
  something.
- **Direction:** `halo effect direction EFFECT reverse|normal` (`reverse_EFFECT` in `halo.conf`)
  runs time backwards in rainbow, cycle, comet and tide.
- **Hardware effects (0 CPU):** `breathing` (base color per zone, or 1–2 custom colors), `cycle`
  and `rainbow` (no palette; direction honored) are animated by **the MCU alone**: the daemon
  programs the animation through report `0x5D` and hands the rings over (LampArray in autonomous
  mode). `halo effect` and `halo status` say "animated by the MCU". When something must be painted
  in software (a game, an external layer, the screen going off), the daemon takes over and gives
  the rings back afterwards; the MCU remembers its program. `halo config hardware_effects no`
  forces software.
- **Battery:** soft effects paint at 25 fps and the strobe only wakes on each change. The rainbow
  costs the daemon about 1 ms of CPU per second. With no effect, only the screen-state read once
  per second remains (below).
- **Safety:** Halo's strobe never exceeds 3 flashes per second (WCAG 2.3.1, photosensitive
  epilepsy). This does not apply to games: the bridges paint what the game asks for, and
  compliance is the game's responsibility.
- The comet's order is tuned to the hardware: on the left ring the top zone comes before the
  bottom one so both rings turn the same way.
- **Each LED lights half a ring:** dimming a zone makes that half look off and the ring looks cut.
  That is why tide and starry move **hue** rather than brightness: tide never drops below 75 % and
  the starry sky is always on.

## Clock (`clock` and `clock12`)
Left ring = hours, right ring = minutes; tens on top, units below. Each digit is a color of the
visible spectrum from red to violet, and **0 is off**:

| 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 |
|---|---|---|---|---|---|---|---|---|---|
| off | red | orange | yellow | lime | green | turquoise | sky blue | blue | violet |
| `000000` | `ff0000` | `ff4000` | `ffaa00` | `a0ff00` | `00ff00` | `00ffa0` | `00a0ff` | `0000ff` | `8c00ff` |

Example: 14:09 is red and lime on the left (top and bottom), off and violet on the right.

It breathes with the seconds: 15 % brightness at the start of each even second and 100 % on each
odd one, with a smooth curve. It uses the system's local time (`localtime_r`), so it follows time
zone changes. The nine colors were calibrated on the hardware to be distinguishable.

## Sun
The color follows the real height of the sun at the configured place, computed with the NOAA
formulas (`effects/sun.rs`). It is normalized against the lowest and highest point of the sun on
that day, so solar midnight is always purple and solar noon always yellow, in any season:

| Sun | Color |
|---|---|
| solar midnight | `2d005a` dark purple |
| night | `0a14aa` deep blue |
| before dawn / after dusk | `006ee6` light blue |
| twilight | `783cc8` lavender |
| rising or setting | `ff3214` red orange |
| morning and afternoon | `ff8200` orange |
| solar noon | `ffd200` bright yellow |

The place comes from `halo.conf` (`latitude`, `longitude`, in degrees, north and east positive).
Without them it uses the equator at the time zone's longitude, and sunrise can be off by some
minutes. Set it with `halo config latitude 20.97` and `halo config longitude -89.62` (or `auto`).
It recomputes every 30 s; the frame only changes when the color moves one step.

## Sensors
Armoury Crate's "Smart". The left ring follows the chip temperature and the right one the CPU
usage, on the hue wheel from green (120°) through yellow to red (0°), always at full brightness:

| | green | yellow | red |
|---|---|---|---|
| left: temperature | 45 °C | 67 °C | 90 °C |
| right: CPU usage | 0 % | 50 % | 100 % |

The temperature comes from `k10temp`'s Tctl (else `amdgpu`'s edge, else `acpitz`) and the usage
from the `cpu` line of `/proc/stat`, compared with the previous read. It reads every 2 s and
launches no process. Without a sensor that half stays green instead of going off. On an APU the
GPU temperature tracks the CPU's, which is why the other ring shows usage.

## Battery
The four LEDs take the color of the charge, from red (10 % or less, 0°) through yellow (55 %) to
green (100 %, 120°). While charging, the flashing zone is mixed 60 % towards white and advances in
the comet's order, one zone every 0.75 s. It brightens rather than darkens, because of the
half-ring rule. "Full" with the charger plugged counts as not charging.

It reads `capacity` and `status` from the first `/sys/class/power_supply/*` with `type = Battery`
(what UPower reads), without D-Bus. Every 10 s when not charging; while charging it wakes on each
step of the flash.

## Pulse
Armoury Crate's "Music". It uses the same capture as the equalizer (`parec` on the default
output's monitor, FFT at 30 frames/s) and listens in two ways at once:

- **Bass (40–160 Hz):** kick and bass. The normal mode.
- **Spectrum:** whichever band stands out at each moment, each with its own gain. For piano,
  guitar or voice.

After **5 s without a bass hit** it follows the spectrum. With **the first bass hit** it comes
back, in that same frame. Both detectors always run, so switching invents no beats.

- The four LEDs are always on, in a hue-wheel color. In silence they stay still.
- The level brightens them towards white, up to 70 %: the ring beats without going dark.
- **Hit:** the level jumps (+0.25 against the previous frame) and exceeds 1.5× its average over
  the last second; then the hue jumps 45°. A sustained sound is not a hit.
- At least 0.35 s between hits, so no more than 2.9 changes per second (WCAG).

**Bounds:** the automatic gain's peak has a floor (0.05). Without it, after about ten minutes of
silence the peak reached 0 and `0/0` left the level at NaN for good (also in the equalizer); and
too low, the window leakage between bands (a 1 kHz tone leaves ~0.0007 in the bass) read as a kick.
All state is fixed-size: buffers are created once, counters saturate and the hue goes modulo 360.
If the daemon fell behind, `parec` blocks on a full pipe (~2 s of audio) and PipeWire drops the
rest: nothing grows.

## Off with the screen
When the internal panel turns off, the rings turn off (games included) and the effects pause: no
frames nobody sees. When it turns on, everything comes back as it was, within 1 s either way.

The state comes from `/sys/class/drm/card*-eDP-*/dpms` (`On`/`Off`), written by the driver, so it
works with KWin and with gamescope alike. It is **read every second** because no usable event
exists: the kernel sends no uevent for DPMS (only for connecting or disconnecting outputs); the
sysfs file does not notify changes, so `poll()`/inotify never wake; neither KWin nor PowerDevil
emit a session-bus signal; and the Wayland protocol `org_kde_kwin_dpms` only exists in KWin.

It costs as good as nothing (no measurable CPU over 30 s), but it does break "no effect, no loop".
To remove it: `screen_off = no` in `halo.conf`, or `halo config screen_off no` (applied at once:
removes or restores the watcher).

## Discarded
- **Adaptive Color** (mirroring the screen's colors). On Wayland only the compositor can hand out
  pixels. Through the ScreenCast portal it works (32×18 quadrants at 5 fps via `pipewiresrc`), but
  it asks permission with a dialog, Plasma shows the "screen shared" notification the whole time,
  and the restore token can fail on devices with a single output. The `org.kde.KWin.ScreenShot2`
  alternative only authorizes processes whose `.desktop` declares
  `X-KDE-DBUS-Restricted-Interfaces`, costs KWin a full render per capture and does not exist in
  game mode. Not worth the cost.
- **AI lighting** (a palette generated by an LLM from a text prompt).
