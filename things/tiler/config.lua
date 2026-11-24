-- Settings for the tiler. Edit, save, and press Reload all.
return {
  -- Pixels between windows, and between windows and the monitor edge.
  gap = 8,

  -- The smallest tile a window gets. A window that would get less, or that
  -- refuses its tile because the app has a larger minimum, joins a stack:
  -- it shares a tile with the windows there, one shown at a time.
  min_width = 400,
  min_height = 300,

  -- Frames drawn above the desktop: green around the focused window, cyan
  -- around floating windows, and a tab strip over each stack.
  border = {
    enabled = true,
    color = "#2f6f5e",
    width = 3,
    float_color = "#00bcd4",
    tab_color = "#7f8c8d",
  },

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
    stack_next = "alt+shift+n",
  },

  -- Dragging a window by its title bar shows where it would go on the tile
  -- under the cursor: near an edge it splits that tile, in the middle third
  -- it swaps with it. Dragging an edge between tiles resizes the split.
}
