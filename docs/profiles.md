# Per-game profiles

Armoury Crate's "scenarios": when a Steam game opens, the rings change to what it was given (a
color, one per zone, an effect or off) and when it closes the usual lights return. Games that send
lights (Chroma, GameSense, Logitech) paint on top of the profile, as always.

## Use
```bash
halo profile                      # the running game and the profiles
halo profile 427520 tide          # when Factorio opens: the tide effect
halo profile 427520 red           # ... or a color
halo profile 427520 red green red green   # one per zone
halo profile 427520 off
halo profile 427520 remove
halo proton list                  # the AppIDs
```
Saved in `halo.conf` as `profile_APPID = VALUE` (also `halo config profile_427520 tide`). In the
window: the Games page, "Running game" (gives whatever is open the current color, off or an
effect) and "Per-game profiles" (the list, and adding one from the installed games). Over D-Bus:
the properties `CurrentGame` (AppID, name), `Profiles` (AppID, name, value) and `InstalledGames`;
changed with `SetConfig("profile_APPID", value)`.

## What the daemon does
- A **color** profile stops whatever effect was running and sets a `profile` layer of priority 120
  (above the ambient effect, below the games). When the game closes it removes the layer and
  restarts the effect saved in `halo.conf` (or the base color).
- An **effect** profile stops the usual effect and starts the profile's one, without saving it.
  When the game closes, the saved one returns. Hardware effects (breathing, cycle, rainbow) still
  go to the MCU.
- Changing the running game's profile applies at once. If the daemon starts with a game already
  open, it detects it.
- While the profile is on, `halo effect X` replaces it and `halo set` changes the base color
  underneath (visible when the game closes).

## How it knows which game runs
Steam does not say over D-Bus and no longer writes `RunningAppID` to `~/.steam/registry.vdf`.
What it does do is rewrite `steamapps/appmanifest_APPID.acf` with `LastPlayed = now` both when
**launching** and when **closing** a game. So:

1. **inotify** on each `steamapps/` (every library in `libraryfolders.vdf`), events
   `IN_CLOSE_WRITE` and `IN_MOVED_TO`. An `appmanifest_*.acf` with `LastPlayed` within 20 s of now
   says "something happened to that AppID". Steam also rewrites manifests when updating or
   verifying games, but then `LastPlayed` does not change.
2. One pass over `/proc/*/cmdline` looking for `reaper SteamLaunch AppId=N` (the process Steam
   launches every game with, native or Proton; it runs as the user). Repeated at 0, 1 and 3 s,
   because the manifest and the reaper appear almost together. If present, the game just opened.
3. A **pidfd** on the reaper (`pidfd_open` + epoll): wakes only when the game ends.

No loops: between events the daemon does nothing. The manifest Steam writes on close is ignored
(the game is already in the list; the pidfd tells). With several games open, the most recent one
wins. Verified with Factorio: 2 ms between "game opened" and the profile's effect starting.

### Discarded
- `RunningAppID` in `registry.vdf`: dead in current Steam.
- Sweeping `/proc` for `SteamAppId=` in process environments: works, but it is a loop, and
  `environ` cannot be read from outside the user. Here `/proc` is only touched on an event.
- Waiting for the game to connect to a bridge: only works for games that send lights, and the
  profile matters precisely for those that do not.
- The cgroup: Steam launches games inside `app-steam@autostart.service`, with no per-game scope.
