# Halo for Decky

The joystick rings of the ROG Xbox Ally from Steam's quick access menu: the current colors, "Rings
off", hardware brightness, the effect and its speed, a row of base colors, and whether games may
paint the rings. In English, Spanish, French and Japanese, following Steam's language.

It needs Halo's daemon running for the user (`systemctl --user status halo`); install Halo from
https://github.com/kirebyte/halo first. The plugin does not need root: it talks to the daemon over
the user's session bus.
