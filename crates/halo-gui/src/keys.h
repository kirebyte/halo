// Key injection for gamepad navigation: posts a key press and release to the focused window.
#pragma once
#include <cstdint>
#include "rust/cxx.h"

namespace halo {
// Sends `key` (a Qt::Key) with `modifiers` (Qt::KeyboardModifiers) to the focused window, on the
// GUI thread. Safe to call from any thread; does nothing while no window of ours has the focus.
// Up and Down go as Backtab and Tab (focus moves) unless a combo box has its list open.
void sendKey(std::int32_t key, std::int32_t modifiers);

// One step of the HALO_GUI_SCRIPT debug script that is not a key: "focus" prints the item with the
// focus, "shot:<path>" saves the window as PNG, "quit" quits. On the GUI thread, like sendKey.
void debugStep(rust::Str step);
}
