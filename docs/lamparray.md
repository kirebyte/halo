# The Xbox Ally rings through HID LampArray

Tested on an RC73YA (BIOS RC73YA.321, Bazzite kernel 7.2).

## Where it is
USB interface **00** of `0B05:1B4C` (a `hidrawN`; the number changes between boots) declares a
standard HID LampArray: usage page `0x59`, encoded as `06 59 00` at the start of the descriptor.
It comes with **mode `0666`**, and InputPlumber does not hide it (it does open it as a source). No
udev rule and no root needed.

## Attributes read
- Array (feature 1): `LampCount=4`, bounding box 300×80×0 mm, `Kind=3` (GameController),
  `MinUpdateInterval=2000 µs`.
- Lamps (feature 2 = request with `LampId` u16, feature 3 = response):

| Lamp | Position (µm) | Ring | Levels | Programmable |
|---|---|---|---|---|
| 0 | 18000, 40000 | left | R/G/B/I 255 | yes |
| 1 | 28000, 16000 | left | 255 | yes |
| 2 | 242000, 40000 | right | 255 | yes |
| 3 | 233000, 64000 | right | 255 | yes |

## Reports (feature)
| ID | Use | Layout |
|---|---|---|
| 1 | LampArrayAttributes (read) | `u16 count, u32 w, h, d, kind, min_interval` |
| 2 | LampAttributesRequest | `u16 lamp_id` |
| 3 | LampAttributesResponse | `u16 id, u32 x, y, z, latency, purposes, u8 R, G, B, I levels, programmable, input_binding` |
| 4 | LampMultiUpdate | `u8 count, u8 flags (bit0 = UpdateComplete), u16 ids[8], u8 RGBI[8][4]` (51 bytes with the ID) |
| 5 | LampRangeUpdate | `u8 flags, u16 start, u16 end, u8 R, G, B, I` |
| 6 | LampArrayControl | `u8 autonomous_mode` |

To set a color: `06 00` (AutonomousMode off), then `04 04 01 00 00 01 00 02 00 03 00 ... RGBI×4`.

## Verified behaviour
1. With **AutonomousMode=0**, LampMultiUpdate sets any color per lamp.
2. With **AutonomousMode=0**, writes through sysfs (`brightness`, which InputPlumber does on
   wake-up) **no longer override** the color.
3. With **AutonomousMode=0**, the vendor command `5A D1 08 0C` (hidraw of interface 02) **is
   ignored**.
4. Back at **AutonomousMode=1**, vendor and sysfs take over again.
5. **Hardware brightness:** the vendor command `5A BA C5 C4 <0-3>` (interface 02) turns the rings
   off at `0` whatever paints them, and `03` is full. Needs the udev rule in `packaging/udev/`.
   Seen to be ignored at times while LampArray paints (2026-11-03), so at `0` the daemon also
   paints every zone off, as it does with the screen off; that holds against games and effects
   until the brightness comes back.
6. **Hardware effects:** the vendor interface accepts the ROG Aura keyboard protocol
   `5D B3 zone mode R G B speed dir 00 R2 G2 B2` followed by `5D B4` (apply). Modes the Ally's MCU
   honours: 0 static, 1 breathe (1–2 colors), 2 cycle, 3 rainbow wave (direction 0/1), 10 pulse;
   4–8, 11 and 12 are ignored. Speeds `E1`/`EB`/`F5`. Zone byte = lamp index + 1. They need
   AutonomousMode=1, and the MCU remembers its program while LampArray paints.
7. Color survives suspend when the daemon re-sends `AutonomousMode=0`, brightness and the frame a
   couple of seconds after `PrepareForSleep(false)`.

## Consequence for the design
The preferred backend is `lamparray`, without root, and neither the driver nor InputPlumber step
on it. The `vendor` backend stays as a fallback and for brightness and MCU effects. When
restoring (at start or after a wake-up) the daemon sends: hardware brightness over vendor, if
accessible; `AutonomousMode=0`; and the frame.

## sysfs, for the record
`/sys/class/leds/ally:rgb:joystick_rings/multi_intensity` takes four values the kernel driver
interprets as packed `0xRRGGBB` colors, but `multi_max_intensity` is 255, so each is clipped to the
blue byte: only blue is reachable this way. Any sysfs write also makes the driver re-send its own
color to the MCU. Halo does not use sysfs.
