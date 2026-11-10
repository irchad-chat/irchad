# Phase 9: Game overlay

**Goal:** a display-only overlay over the focused game: who's in our voice call and who's speaking, and the last messages from channels the user picks. Never injected into the game (D74).

Decisions: D63 (game list), D74 (see-through window, Windows and X11), D75 (Wayland layer surface).

- [x] Settings page: enable, show/hide combo, corner, voice and messages, channels, line count and fade, background opacity, the user's own executables, Preview.
- [x] Focus: Windows `GetForegroundWindow`, X11 `_NET_ACTIVE_WINDOW` → `_NET_WM_PID`, matched against Discord's game list and the user's executables.
- [x] Windows: an eframe viewport, DX12 through DirectComposition, the redirection surface color-keyed out.
- [x] X11: an always-on-top, click-through, see-through eframe viewport.
- [x] Wayland: a `wlr-layer-shell` surface in the focused output's corner, with its own egui context and renderer (D75). Focus from XWayland, and from Hyprland's IPC on Hyprland (it doesn't clear `_NET_ACTIVE_WINDOW`).
- [ ] Check on X11 hardware: over a borderless game, clicks go through, toggle combo, all four corners.
- [ ] Check on Wayland hardware (KDE, Hyprland, Sway): Preview, over `glxgears -fullscreen` added under "Your games" and a Proton game, hides on Alt+Tab to a Wayland window, GNOME shows the no-layer-shell message.
- [ ] Wayland: the overlay is drawn from the main window's `ui`; check that it keeps updating while IrChad's window is on another workspace.
- [x] Wayland: avatars (register the main renderer's textures with `register_native_texture`).
- [x] Wayland: system fallback fonts (CJK) in the layer's egui context.
- [ ] Wayland: fractional scale (`wp_fractional_scale_v1`); it draws at the scale rounded up now.
- [ ] Wayland: place it in a windowed game's corner, not the output's (map `Target::rect` through xdg-output).
- [ ] Wayland: native-Wayland games on Sway (i3 IPC) and KWin (a script reporting `workspace.activeWindow`).
- [ ] Gamescope: its overlay atom.

## Done when
- On Windows, X11 and a layer-shell Wayland desktop, the overlay shows over a borderless game while it has focus, hides when it doesn't, passes every click through, and shows voice and picked channels' messages.
