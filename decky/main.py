"""Halo's Decky plugin, backend half: a thin bridge between the panel and Halo's daemon
(`org.halo.Halo1` on the user's session bus), through `busctl`. The plugin has no `root` flag, so
Decky runs this as the user and the session bus is simply theirs.

Every method is called from `src/index.tsx` with `callable`. The setters return an error message
("" when all went well) so the panel can show it in a toast; `state` returns what the panel
shows. Translations come from Halo's own `locales/*.json`, copied in by `build.sh`.
"""

import asyncio
import json
import os
import pwd

import decky

BUS = "org.halo.Halo1"
PATH = "/org/halo/Halo1"
IFACE = "org.halo.Halo1"
PROPERTIES = "org.freedesktop.DBus.Properties"
TIMEOUT = "5"


def _env() -> dict[str, str]:
    """The session bus of the user, whatever Decky left in the environment."""
    env = dict(os.environ)
    try:
        uid = pwd.getpwnam(decky.DECKY_USER).pw_uid
    except (KeyError, AttributeError):
        uid = os.getuid()
    env.setdefault("XDG_RUNTIME_DIR", f"/run/user/{uid}")
    env.setdefault("DBUS_SESSION_BUS_ADDRESS", f"unix:path=/run/user/{uid}/bus")
    return env


def _plain(value):
    """Strips busctl's `{"type": …, "data": …}` wrappers, recursively."""
    if isinstance(value, dict) and set(value) == {"type", "data"}:
        return _plain(value["data"])
    if isinstance(value, list):
        return [_plain(v) for v in value]
    if isinstance(value, dict):
        return {k: _plain(v) for k, v in value.items()}
    return value


async def _busctl(*args: str):
    proc = await asyncio.create_subprocess_exec(
        "busctl", "--user", "--json=short", f"--timeout={TIMEOUT}", *args,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=_env(),
    )
    out, err = await proc.communicate()
    if proc.returncode != 0:
        message = err.decode(errors="replace").strip() or f"busctl exited with {proc.returncode}"
        raise RuntimeError(message)
    text = out.decode(errors="replace").strip()
    return _plain(json.loads(text)) if text else None


async def _call(method: str, signature: str = "", *args: str):
    """Calls a method of the daemon; the returned values, unwrapped."""
    extra = [signature, *args] if signature else []
    result = await _busctl("call", BUS, PATH, IFACE, method, *extra)
    return result if result else []


async def _properties() -> dict:
    result = await _busctl("call", BUS, PATH, PROPERTIES, "GetAll", "s", IFACE)
    return result[0] if result else {}


async def _attempt(method: str, signature: str = "", *args: str) -> str:
    """Runs a setter and turns its failure into the message the panel shows."""
    try:
        await _call(method, signature, *args)
        return ""
    except Exception as e:  # noqa: BLE001 - whatever busctl or the daemon said
        decky.logger.warning("%s failed: %s", method, e)
        return str(e)


class Plugin:
    async def state(self) -> dict:
        """What the panel shows; `connected: False` with the error when the daemon is not there."""
        try:
            p = await _properties()
        except Exception as e:  # noqa: BLE001
            return {"connected": False, "error": str(e)}
        game = p.get("CurrentGame", ["", ""])
        return {
            "connected": True,
            "effect": p.get("Effect", ""),
            "effects": p.get("Effects", []),
            "speed": p.get("EffectSpeed", "medium"),
            "hardware": bool(p.get("EffectHardware", False)),
            "brightness": int(p.get("Brightness", 3)),
            "base": p.get("Base", []),
            "frame": p.get("Frame", []),
            "games": bool(p.get("GamesEnabled", True)),
            "game": [game[0], game[1]] if len(game) == 2 else ["", ""],
        }

    async def set_zones(self, colors: list[str]) -> str:
        """One "rrggbb", or one per zone (left bottom, left top, right top, right bottom)."""
        return await _attempt("SetZones", "as", str(len(colors)), *colors)

    async def set_brightness(self, level: int) -> str:
        return await _attempt("SetBrightness", "y", str(max(0, min(3, int(level)))))

    async def start_effect(self, name: str) -> str:
        return await _attempt("StartEffect", "s", name)

    async def stop_effect(self) -> str:
        return await _attempt("StopEffect")

    async def set_speed(self, speed: str) -> str:
        return await _attempt("SetEffectSpeed", "s", speed)

    async def set_games_enabled(self, on: bool) -> str:
        return await _attempt("SetGamesEnabled", "b", "true" if on else "false")

    async def set_config(self, key: str, value: str) -> str:
        return await _attempt("SetConfig", "ss", key, value)

    async def translations(self, lang: str) -> dict:
        """Halo's strings for `lang` ("es", "fr", "ja"); empty for English (the keys are English)."""
        if not lang.isalpha():
            return {}
        path = os.path.join(decky.DECKY_PLUGIN_DIR, "locales", f"{lang}.json")
        try:
            with open(path, encoding="utf-8") as f:
                return json.load(f)
        except OSError:
            return {}

    async def _main(self):
        decky.logger.info("Halo plugin up; talking to %s on the user's session bus", BUS)

    async def _unload(self):
        pass

    async def _uninstall(self):
        pass
