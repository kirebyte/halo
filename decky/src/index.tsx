// Halo's Decky plugin, panel half: the ROG Xbox Ally joystick rings from the quick access menu.
// Everything goes through `main.py`, which talks to Halo's daemon over D-Bus; the panel polls the
// state while it is open and after every change.
import {
  DropdownItem,
  Field,
  Focusable,
  PanelSection,
  PanelSectionRow,
  SliderField,
  ToggleField,
  staticClasses,
} from "@decky/ui";
import { callable, definePlugin, toaster } from "@decky/api";
import { useEffect, useRef, useState } from "react";
import { FaCircleNotch } from "react-icons/fa";

type State = {
  connected: boolean;
  error?: string;
  effect: string;
  effects: string[];
  speed: string;
  hardware: boolean;
  brightness: number;
  base: string[];
  frame: string[];
  games: boolean;
  game: [string, string];
};

const getState = callable<[], State>("state");
const setZones = callable<[colors: string[]], string>("set_zones");
const setBrightness = callable<[level: number], string>("set_brightness");
const startEffect = callable<[name: string], string>("start_effect");
const stopEffect = callable<[], string>("stop_effect");
const setSpeed = callable<[speed: string], string>("set_speed");
const setGamesEnabled = callable<[on: boolean], string>("set_games_enabled");
const translations = callable<[lang: string], Record<string, string>>("translations");

// The same swatches as the window's color picker.
const SWATCHES = ["ff0000", "ff8000", "ffff00", "00ff00", "00ffff", "0000ff", "8000ff", "ff00ff", "ff80c0", "ffffff", "000000"];
const SPEEDS = ["slow", "medium", "fast"];
const POLL_MS = 1500;

// Halo's own translations (locales/*.json): the key is the English text.
let dict: Record<string, string> = {};
function t(key: string): string {
  return dict[key] ?? key;
}
function language(): string {
  const lang = (navigator.language || "en").toLowerCase();
  for (const code of ["es", "fr", "ja"]) {
    if (lang.startsWith(code)) return code;
  }
  return "en";
}

function Swatch({ color, selected, onPick }: { color: string; selected: boolean; onPick: () => void }) {
  const [focused, setFocused] = useState(false);
  return (
    <Focusable
      onActivate={onPick}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
      style={{
        width: 28,
        height: 28,
        borderRadius: 14,
        margin: 4,
        boxSizing: "border-box",
        background: "#" + color,
        border: focused ? "3px solid #fff" : selected ? "3px solid #8a8a8a" : "3px solid #3a3a3a",
        transform: focused ? "scale(1.2)" : "none",
        transition: "transform 0.1s",
      }}
    >
      {null}
    </Focusable>
  );
}

function Dot({ color }: { color: string }) {
  return <span style={{ display: "inline-block", width: 14, height: 14, borderRadius: 7, margin: "0 2px", background: "#" + color, border: "1px solid #555" }} />;
}

function Content() {
  const [state, setState] = useState<State | null>(null);
  // The brightness to come back to when "Rings off" is switched off.
  const lastBrightness = useRef(3);

  const refresh = async () => {
    try {
      const s = await getState();
      if (s.connected && s.brightness > 0) lastBrightness.current = s.brightness;
      setState(s);
    } catch (e) {
      setState({ connected: false, error: String(e) } as State);
    }
  };

  useEffect(() => {
    let alive = true;
    (async () => {
      dict = await translations(language());
      if (alive) await refresh();
    })();
    const timer = setInterval(refresh, POLL_MS);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);

  // Runs a setter; the daemon's complaint, if any, goes to a toast.
  const act = async (result: Promise<string>) => {
    const error = await result;
    if (error) toaster.toast({ title: "Halo", body: error });
    await refresh();
  };

  if (!state) {
    return (
      <PanelSection title={t("Lights")}>
        <PanelSectionRow>
          <Field label="…" />
        </PanelSectionRow>
      </PanelSection>
    );
  }
  if (!state.connected) {
    return (
      <PanelSection title={t("Lights")}>
        <PanelSectionRow>
          <Field label={t("No daemon")} description={t("The Halo daemon is not running: systemctl --user start halo")} />
        </PanelSectionRow>
      </PanelSection>
    );
  }

  const effectOptions = [{ data: "none", label: t("none") }, ...state.effects.map((e) => ({ data: e, label: t("effect." + e) }))];
  const speedOptions = SPEEDS.map((s) => ({ data: s, label: t("speed." + s) }));
  const base = state.base[0] ?? "";
  const oneColor = state.base.every((c) => c === base);

  return (
    <>
      <PanelSection title={t("Lights")}>
        <PanelSectionRow>
          <Field label={t("The current color")} bottomSeparator="none">
            {state.frame.map((c, i) => (
              <Dot key={i} color={c} />
            ))}
          </Field>
        </PanelSectionRow>
        <PanelSectionRow>
          <ToggleField
            label={t("Rings off")}
            description={t("Turns the rings off without losing the color or the effect")}
            checked={state.brightness === 0}
            onChange={(on) => act(setBrightness(on ? 0 : lastBrightness.current))}
          />
        </PanelSectionRow>
        <PanelSectionRow>
          <SliderField
            label={t("Hardware brightness")}
            value={state.brightness}
            min={0}
            max={3}
            step={1}
            notchCount={4}
            notchLabels={[
              { notchIndex: 0, label: t("off"), value: 0 },
              { notchIndex: 1, label: "1", value: 1 },
              { notchIndex: 2, label: "2", value: 2 },
              { notchIndex: 3, label: "3", value: 3 },
            ]}
            onChange={(v) => act(setBrightness(Math.round(v)))}
          />
        </PanelSectionRow>
      </PanelSection>

      <PanelSection title={t("Effect")}>
        <PanelSectionRow>
          <DropdownItem
            label={t("Effect")}
            rgOptions={effectOptions}
            selectedOption={state.effect || "none"}
            onChange={(o) => act(o.data === "none" ? stopEffect() : startEffect(o.data))}
          />
        </PanelSectionRow>
        <PanelSectionRow>
          <DropdownItem
            label={t("Speed")}
            rgOptions={speedOptions}
            selectedOption={state.speed}
            disabled={!state.effect}
            onChange={(o) => act(setSpeed(o.data))}
          />
        </PanelSectionRow>
      </PanelSection>

      <PanelSection title={t("Base color")}>
        <PanelSectionRow>
          <Focusable style={{ display: "flex", flexWrap: "wrap", justifyContent: "center", padding: "4px 0" }} flow-children="horizontal">
            {SWATCHES.map((c) => (
              <Swatch key={c} color={c} selected={oneColor && base === c} onPick={() => act(setZones([c]))} />
            ))}
          </Focusable>
        </PanelSectionRow>
      </PanelSection>

      <PanelSection title={t("Games")}>
        <PanelSectionRow>
          <ToggleField label={t("Games control the lights")} checked={state.games} onChange={(on) => act(setGamesEnabled(on))} />
        </PanelSectionRow>
        {state.game[1] ? (
          <PanelSectionRow>
            <Field label={t("Running game")} description={state.game[1]} />
          </PanelSectionRow>
        ) : null}
      </PanelSection>
    </>
  );
}

export default definePlugin(() => {
  return {
    name: "Halo",
    titleView: <div className={staticClasses.Title}>Halo</div>,
    content: <Content />,
    icon: <FaCircleNotch />,
    onDismount() {},
  };
});
