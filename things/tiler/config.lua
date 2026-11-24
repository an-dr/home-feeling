-- Settings for the tiler. Edit, save, and press Reload all.
return {
  -- Pixels between windows, and between windows and the monitor edge.
  gap = 8,

  -- A new window whose tile would be narrower or shorter than this floats
  -- at its own size instead, and the other tiles stay as they are.
  min_width = 400,
  min_height = 300,

  -- The border around the focused window.
  border = { enabled = true, color = "#2f6f5e", width = 3 },

  -- Hotkeys: modifiers alt, ctrl, shift, win, joined with + to one key.
  -- Windows keeps most win+ combinations for itself.
  keys = {
    focus_left = "alt+left",
    focus_right = "alt+right",
    focus_up = "alt+up",
    focus_down = "alt+down",
    swap_left = "alt+shift+left",
    swap_right = "alt+shift+right",
    swap_up = "alt+shift+up",
    swap_down = "alt+shift+down",
    toggle_float = "alt+shift+space",
  },
}
