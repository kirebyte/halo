# Game bridges

On Windows, games drive lighting during gameplay through the vendors' SDKs; the most used one is
**Razer Chroma**. Halo receives those effects from Proton/Wine games and paints them on the rings as
a layer of priority 200: above the base color and the equalizer. When the game quits, the layer
disappears and what was there before returns.

`halo games off` stops painting what games send (Chroma and GameSense) without cutting their
connection, and `halo games on` brings it back at once. Saved in `halo.conf` (`games = yes|no`).

**Per-game permission:** `halo games` lists the connected games (with their bridge) and the blocked
ones; `halo games block GAME` and `halo games allow GAME` decide per game, by the name the list
shows, case-insensitively. The name is what the game tells the bridge: the title it sends to
Chroma's `Init` or REST `POST`, GameSense's `game`, or the name Logitech registers; it is what goes
between `:` and `#` in the layer (`chroma:Factorio#3`). A blocked game stays connected, it just is
not painted, so allowing it shows at once. Saved as `block = GAME` (one line per game). Over D-Bus:
the `Games` property and `SetGameAllowed(game, bool)`. OpenRGB clients are tools, not games, and do
not take part in this.

| SDK | Status |
|---|---|
| Razer Chroma (native DLL) | verified with a real game (Factorio under Proton) |
| Razer Chroma (REST, `localhost:54235`) | verified from a browser with `tools/chroma-rest.html` |
| SteelSeries GameSense (`:54237`) | verified with a real game (Factorio: orange, red on damage) |
| OpenRGB SDK (`:6742`) | verified with `openrgb-python` (negotiates protocol 4) |
| Logitech LED SDK (DLL + registry, `:54238`) | only with the old wrapper; the new one checks the DLL signature (below) |
| ASUS Aura SDK (COM `aura.sdk.1`) | postponed: almost no game uses it |
| Windows Dynamic Lighting | out of scope: Wine does not implement `Windows.Devices.Lights` |

## Which games send lights?
```bash
halo proton scan           # every installed game
halo proton scan 692850    # one
```
It searches the game's `.exe` and `.dll` files (ASCII and UTF-16) for each SDK's fingerprints: the
Chroma DLL names, the `/razer/chromasdk` URL, `LogiLedInit` and Logitech's CLSID, `coreProps.json`
and GameSense's `bind_game_event`. It also flags SDKs Halo does not cover (Game State Integration,
iCUE, ASUS Aura SDK, AlienFX). Public lists are incomplete, so this is the most reliable check; an
encrypted executable (Denuvo, etc.) can still hide its strings.

## Razer Chroma

### Use
```bash
halo proton                  # Steam games with a Proton prefix and whether they have the bridge
halo proton install 1091500  # or --all
halo proton remove 1091500   # only removes Halo's DLLs; never Razer's
```
`install.sh` builds the DLL with mingw-w64 and leaves it in `~/.local/share/halo/chroma/{x64,x86}`.
`halo proton install` copies it into the prefix (`compatdata/<appid>/pfx/drive_c/windows/`):
- `system32`: `RzChromaSDK64.dll` and `RzChromatic64.dll`;
- `syswow64`: `RzChromaSDK.dll` and `RzChromatic.dll`.

Older games look for `RzChromaSDK*`; games built with Razer's newer sample library
(`CChromaEditor`) look for `RzChromatic*`. The prefix exists once the game has run once.

### How it works
```
game (Proton) ──Chroma C API──► Halo's RzChromaSDK64.dll ──TCP 127.0.0.1:54236──► halo daemon ──► layer "chroma:<game>#<port>"
```
- The DLL (`shims/chroma/chroma.c`) exports the 18 functions Razer's loader requires; if one is
  missing, the game's `Init` fails. Signatures and structs come from the public headers of
  `razerofficial/CChromaEditor` (`RzChromaSDKTypes.h`, `RzChromaSDK.h`, `ChromaSDKPluginTypes.h`).
- It keeps the effects (`Create*Effect`) inside the game. On each `SetEffect` (or a `Create*`
  without an id, which applies at once) it sends `device · type · struct bytes`. **The DLL
  interprets no colors:** all the mapping lives in Rust (`daemon/bridges/chroma.rs`), with tests.
- If the daemon is absent, the DLL retries every 2 s without blocking the game and always answers
  `RZRESULT_SUCCESS`.
- Under Wine and Proton the loopback is the host's: nothing else is needed to reach the daemon,
  and the TCP connection passes through pressure-vessel.

### Protocol
Little endian. Each message is `u32 length` (of what follows), `u8 op` and the data:

| op | Data |
|---|---|
| 1 HELLO | app name in UTF-8 (`InitSDK`'s `Title`, or the .exe name) |
| 2 APPLY | `u8 device`, `u32 effect type`, bytes of Razer's struct |

Devices use Razer's `DEVICE_INFO_TYPE`: 1 keyboard, 2 mouse, 3 headset, 4 mousepad, 5 keypad,
8 ChromaLink. 255 is the generic `CreateEffect` (only `CHROMA_STATIC`). Closing the socket means
the game ended.

### Mapping to the four zones
Zones by lamp position: 0 left bottom, 1 left top, 2 right top, 3 right bottom (`lamparray.md`).

| Effect | How it is painted |
|---|---|
| ChromaLink `CUSTOM` (5 LEDs) | LEDs 1–4 → zones 1–4. If only LED 0 (the "background") is set, it goes everywhere |
| `STATIC` of any device | the same color on all four |
| Keyboard `CUSTOM` 6×22, `CUSTOM_KEY`, `CUSTOM2` 8×24 | keyboard quadrants → zones. The average of the **lit** keys (games usually light a few keys over black). `CUSTOM_KEY` takes `Key` where it carries `0x01000000` |
| Mouse `CUSTOM2` 9×7, keypad 4×5 | like the keyboard |
| Mouse `CUSTOM` (30), headset `CUSTOM` (5) | average on all four |
| Mousepad `CUSTOM` (15/20) | first half → left ring, second → right |
| `CHROMA_NONE` | that device goes off |
| breathing, wave, reactive, spectrum cycling... | ignored (they were animations of Razer's firmware) |

If the game paints several devices, one is used, in this order: ChromaLink, keyboard, headset,
mousepad, mouse, keypad, generic.

### Known limits
- **DLL signature:** Razer's sample loader can check that the DLL is signed by "Razer USA Ltd."
  (`VerifyLibrarySignature.cpp`), but only if the game was built with
  `CHECK_CHROMA_LIBRARY_SIGNATURE`. Those games will reject Halo's DLL; for them only REST, or
  nothing, remains. The **version** check (FileVersion ≥ 1.0.0.36) passes: the `VERSIONINFO` says
  1.0.0.99.

## Razer Chroma over REST
Same SDK, over HTTP at `http://localhost:54235/razer/chromasdk`. Used by web apps and some games
(`daemon/bridges/chroma_rest.rs`).
- `GET` gives the version. A `POST` with `title` creates a session and answers
  `{"sessionid": N, "uri": "http://localhost:54235/chromasdk/N"}`. Synapse opens one port per
  session; Halo uses the same port with the path, and clients use the `uri` as given.
- Under the `uri`: `PUT`/`POST` to `keyboard`, `mouse`, `headset`, `mousepad`, `keypad` and
  `chromalink`; `PUT`/`DELETE` to `effect` with `id` or `ids`; `PUT heartbeat`; `DELETE` on the
  `uri` to close. `PUT` paints at once. `POST` only creates the effect and returns its `id`, and
  also accepts `effects` with several.
- The JSON is translated into the same C structs the DLL sends and goes through the same `decode`,
  so the zone mapping is the table above. The REST docs call the 20-LED mousepad `CHROMA_CUSTOM`
  (`CUSTOM2` in C): detected by length.
- A session closes after 15 s without requests. Each session is the layer `chroma:<title>#rest<N>`,
  priority 200 and under the `halo games` switch.
- **Open CORS** (`Access-Control-Allow-Origin: *`), like Razer's server: the API is meant for web
  pages. GameSense, by contrast, rejects `Origin`, since only games talk to it.
- Not implemented: `/devid=...` (effects for an exact Razer model) answers `50` (unsupported), and
  the HTTPS on `chromasdk.io:54236` does not exist (that port is the DLL bridge).

### Test page
`tools/chroma-rest.html` opens a session, sends heartbeats and tries ChromaLink per zone, static,
a 20 fps rainbow, a flashing "damage" and a 6×22 keyboard. Serve it from localhost while testing
(a sandboxed browser may not read the file directly):
```bash
python3 -m http.server 8765 --bind 127.0.0.1 --directory tools
# then open http://127.0.0.1:8765/chroma-rest.html
```

## SteelSeries GameSense

### Use
`halo proton install` also writes
`drive_c/ProgramData/SteelSeries/SteelSeries Engine 3/coreProps.json` in the prefix with
`{"address":"127.0.0.1:54237","halo":true}`. That is how the game finds it. If one without `"halo"`
exists (a real SteelSeries Engine) it is left alone, and `halo proton remove` only deletes ours.

### How it works
```
game (Proton) ──reads coreProps.json──► HTTP POST JSON 127.0.0.1:54237 ──► halo daemon ──► layer "gamesense:<GAME>"
```
- A minimal HTTP/1.1 of its own (`bridges/http.rs`): keep-alive, `Expect: 100-continue`, chunked.
  It rejects any request with an `Origin` header with 403: games do not send it, and this way a
  web page cannot paint the rings.
- Endpoints: `game_metadata`, `register_game_event`, `bind_game_event`, `game_event`,
  `multiple_game_events`, `supports_multiple_game_events`, `game_heartbeat`, `stop_game`,
  `remove_game_event` and `remove_game`. `load_golisp_handlers` and `register_game_event_icon`
  are accepted and do nothing. Errors like Engine's: 400 with `{"error": "..."}`.
- Handlers are saved in `~/.local/state/halo/gamesense.json`, because games register them once
  and do not repeat them at every start.
- A game goes off after 15 s without events (or its `deinitialize_timer_length_ms`), or with
  `stop_game`. No loop: one task sleeps until the next deadline.

### Mapping to the four zones
| Handler (`device-type` / `zone`) | Zones |
|---|---|
| `rgb-1-zone` · `one` | all four |
| `rgb-2-zone` · `one` / `two` | left ring (0,1) / right ring (2,3) |
| `rgb-zoned-device`, `rgb-3..N-zone` · `one`..`four` | zone 0..3 |
| `keyboard`, `mouse`, `headset`, `indicator` (any zone) | all four |
| `rgb-per-key-zones` with `bitmap`/`partial-bitmap` | quadrants of the 22×6 bitmap, average of what is lit |

If two handlers land on the same zone, the more specific one paints on top: zones > bitmap >
keyboard, mouse and headset. Among equals the most recent event wins. Colors: static (black at
value 0, like Engine), `gradient` (0 to 100), `low`/`high` ranges and `context-color` (from the
`frame`). The `percent` and `count` modes are painted as `color`, because with one LED per zone
there is no bar to fill.

`rate` flashes the zone between its color and black: `frequency` cycles per second (fixed or by
ranges, capped at 30 Hz) and, with `repeat_limit`, only that many cycles before staying on. The
cycle restarts when the frequency or limit matching the value changes, not on every repeated
event. No loop: the daemon wakes only on each on/off change.

### Known limits
- GoLisp, screen and tactile handlers are accepted and ignored.

## Logitech LED SDK

### Use
`halo proton install` also installs it, with the game **closed**: Wine rewrites the registry when
the prefix shuts down and the key would be lost. It does two things:
- copies `LogitechLed.dll` to `system32` (64-bit) and `syswow64` (32-bit);
- in `pfx/system.reg`, points the default value of
  `HKLM\Software\Classes\CLSID\{a6519e67-7632-4375-afdf-caa889744403}\ServerBinary` to
  `C:\windows\system32\LogitechLed.dll`, and the 32-bit view
  (`Software\Classes\Wow6432Node\CLSID\...`) to the `syswow64` one.

If the key already points to another DLL (G HUB, Artemis...) it is left alone, and
`halo proton remove` only deletes the key if it points to ours.

### Limit: the new wrapper checks the signature
Games with the recent SDK carry the wrapper statically linked in the .exe and, before loading the
`ServerBinary` DLL, run it through `WinVerifyTrust` and compare the signer (`CryptQueryObject` +
`CertGetNameStringW`) with `"Logitech Inc"`. Halo's DLL is not signed by Logitech, so the game
silently discards it: no connection to the daemon. This cannot be worked around without forging
the signature or disabling Wine's verification, and we do not. `halo proton scan` marks these
games with "Logitech: requires signature". Many of them also carry Chroma or GameSense, which do
work.

### How it works
```
game ──LogitechLedEnginesWrapper.dll (shipped by the game)──reads ServerBinary──► Halo's LogitechLed.dll ──TCP 127.0.0.1:54238──► halo daemon ──► layer "logitech:<game>#<port>"
```
- Games do not call Logitech directly: they ship the SDK's wrapper, which looks up the Logitech
  software's DLL in the registry and loads it. The same trick Artemis and LogiLed2Corsair use.
- The DLL (`shims/logitech/logitech.c`) exports the 31 `LogiLed*`/`LogiGetConfigOption*`
  functions Artemis's `LogitechLed.dll` has. All `cdecl`, all answer yes.
- Protocol: `u32 length · u8 op · data`. `op 1 HELLO` carries the name (from
  `LogiLedInitWithName` or the .exe). `op 2 CALL` carries `u8 function · u8 n · i32 args[n] ·
  bitmap`. The DLL interprets no colors.

### Mapping to the four zones
| Call | How it is painted |
|---|---|
| `SetLighting(r, g, b)` (percentages) | the same color on all four |
| `SetLightingFromBitmap` (21×6 BGRA) | keyboard quadrants → zones, average of what is lit |
| `SetLightingForTargetZone` | the color on all four: the zone number says nothing about the rings |
| `SetLightingForKeyWith*` | only if nothing else: average of the keys on all four |
| `FlashLighting(..., duration, interval)` | flashes with the color: on for half of each interval. Afterwards what was there returns |
| `PulseLighting(..., duration, interval)` | fades to black and back once per interval, at ~30 fps. Duration 0 = until `StopEffects` |
| `SaveCurrentLighting` / `RestoreLighting` | saves and restores what was painted |
| single-key effects, configuration options | accepted and do nothing |

While no effect is active the session does not wake: the pulse runs at ~30 fps and the flash wakes
only on each change.

### Test
`make -C shims/logitech test` builds `test.exe`, which reads the registry key, loads the DLL,
checks the 31 functions and sends magenta, a bitmap, a flash and a pulse. Run it under Wine in a
prefix with `halo proton install`.

## OpenRGB SDK
A server on `127.0.0.1:6742` with protocol 5; serves clients from 0 to 5. It exposes one device,
`ROG Xbox Ally` (gamepad type, vendor ASUS) with:
- one `Direct` mode;
- two linear zones, `Left ring` and `Right ring`, of 2 LEDs each;
- 4 named LEDs (`Left bottom`, `Left top`, `Right top`, `Right bottom`).

It answers `REQUEST_CONTROLLER_COUNT/DATA`, `REQUEST_PROTOCOL_VERSION` and the profile and plugin
lists (empty), and paints with `UPDATELEDS`, `UPDATEZONELEDS` and `UPDATESINGLELED`. What each
client sends is the layer `openrgb:<client>#<port>` of priority 150, above the equalizer and below
the games. It goes away when the client disconnects. If OpenRGB itself is running the port is taken
and the daemon carries on without this server (it says so in the journal).

## Native Linux games: the controller LED
Discarded for now. Native games can only ask for a color through the controller LED
(`SDL_SetGamepadLED`, `ISteamInput::SetLEDColor`), and on the Ally the controller is virtual, from
InputPlumber:
- The `CompositeDevice` advertises `LED:Color` and `LED:Brightness` as outputs and sends them to
  the sysfs LED `ally:rgb:joystick_rings`, where the driver only knows blue.
- The virtual controller is `deck-uhid` (Valve Steam Deck Controller, Bazzite's OpenGamepadUI
  profile). The Deck's protocol has no LED, so SDL and Steam Input send nothing: there is no event
  to intercept. With a virtual DualSense (`ds5`) it would arrive, but end in the blue sysfs, and
  the controller type is the system's decision, not Halo's.
- InputPlumber does not expose the color over D-Bus (`org.shadowblip.Input.Source.LEDDevice` only
  has `Id`), and sysfs does not notify writes.

The path, if it ever matters: propose upstream in InputPlumber a signal or property with the
requested color (or a `dbus` target forwarding `LED:Color`), and have Halo listen to it as a game
layer.
