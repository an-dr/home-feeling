-- The tiler: a dwindle layout per monitor (UC-TILE-01 to 05).
--
-- Each monitor holds a tree. A leaf is one window; a split divides its area
-- between two children, side by side ("h") or stacked ("v"). A new window
-- splits the focused tile along its longer side. Everything happens through
-- the desktop module's bus topics.

local QUERY = "desktop/windows/query"

local windows = {}  -- id -> window info, as the desktop module reports it
local floating = {} -- id -> true for windows left at their own size
local trees = {}    -- monitor id -> root node
local monitors = {} -- monitor id -> monitor
local placed = {}   -- id -> rect the last layout gave it
local focused = nil

local function key(id)
  -- Ids are integers; keep them as table keys of one type.
  return tostring(id)
end

local function setting(name, default)
  local value = config[name]
  if value == nil then return default end
  return value
end

local GAP = setting("gap", 8)
local MIN_W = setting("min_width", 400)
local MIN_H = setting("min_height", 300)
local BORDER = setting("border", { enabled = true, color = "#2f6f5e", width = 3 })

-- Layout -------------------------------------------------------------------

local function split_rect(rect, dir)
  if dir == "h" then
    local wa = (rect.w - GAP) // 2
    return { x = rect.x, y = rect.y, w = wa, h = rect.h },
           { x = rect.x + wa + GAP, y = rect.y, w = rect.w - wa - GAP, h = rect.h }
  end
  local ha = (rect.h - GAP) // 2
  return { x = rect.x, y = rect.y, w = rect.w, h = ha },
         { x = rect.x, y = rect.y + ha + GAP, w = rect.w, h = rect.h - ha - GAP }
end

local function layout(node, rect, out)
  if node.window then
    out[node.window] = rect
    return
  end
  local a, b = split_rect(rect, node.split)
  layout(node.a, a, out)
  layout(node.b, b, out)
end

local function area(monitor)
  local work = monitor.work
  return { x = work.x + GAP, y = work.y + GAP, w = work.w - 2 * GAP, h = work.h - 2 * GAP }
end

local function find(node, id, parent)
  if node.window then
    if node.window == id then return node, parent end
    return nil
  end
  local found, owner = find(node.a, id, node)
  if found then return found, owner end
  return find(node.b, id, node)
end

local function last_leaf(node)
  while not node.window do node = node.b end
  return node
end

local function apply()
  placed = {}
  for monitor_id, tree in pairs(trees) do
    local monitor = monitors[monitor_id]
    if monitor then
      layout(tree, area(monitor), placed)
    end
  end
  for id, rect in pairs(placed) do
    local window = windows[id]
    if window then
      bones.publish("desktop/windows/move", { id = window.id, rect = rect })
    end
  end
end

-- Focus border -------------------------------------------------------------

local function update_border()
  if not BORDER.enabled then return end
  local window = focused and windows[focused]
  if not window then
    bones.publish("desktop/overlay/clear", { id = "focus" })
    return
  end
  local rect = placed[focused] or window.rect
  bones.publish("desktop/overlay/set", {
    id = "focus", rect = rect, color = BORDER.color, width = BORDER.width,
  })
end

-- Tiling -------------------------------------------------------------------

local function monitor_of(window)
  local id = key(window.monitor)
  if monitors[id] then return id end
  return next(monitors)
end

-- Adds a window to its monitor's tree, or floats it when its tile would be
-- smaller than the minimum (UC-TILE-02). Returns whether it was tiled.
local function insert(id)
  local window = windows[id]
  local monitor_id = monitor_of(window)
  if not monitor_id then return false end
  local tree = trees[monitor_id]
  if not tree then
    local rect = area(monitors[monitor_id])
    if rect.w < MIN_W or rect.h < MIN_H then
      floating[id] = true
      return false
    end
    trees[monitor_id] = { window = id }
    floating[id] = nil
    return true
  end
  local target = (focused and focused ~= id and find(tree, focused)) or last_leaf(tree)
  local rects = {}
  layout(tree, area(monitors[monitor_id]), rects)
  local rect = rects[target.window]
  local dir = rect.w >= rect.h and "h" or "v"
  local _, new = split_rect(rect, dir)
  if new.w < MIN_W or new.h < MIN_H then
    floating[id] = true
    bones.debug("floating " .. window.title .. ": its tile would be " .. new.w .. "x" .. new.h)
    return false
  end
  target.split, target.a, target.b = dir, { window = target.window }, { window = id }
  target.window = nil
  floating[id] = nil
  return true
end

local function remove(id)
  for monitor_id, tree in pairs(trees) do
    local leaf, parent = find(tree, id)
    if leaf then
      if not parent then
        trees[monitor_id] = nil
      else
        local sibling = parent.a == leaf and parent.b or parent.a
        parent.window, parent.split, parent.a, parent.b = sibling.window, sibling.split, sibling.a, sibling.b
      end
      return true
    end
  end
  return false
end

local function tiled(id)
  for _, tree in pairs(trees) do
    if find(tree, id) then return true end
  end
  return false
end

-- Directions ---------------------------------------------------------------

local DIRS = { left = { -1, 0 }, right = { 1, 0 }, up = { 0, -1 }, down = { 0, 1 } }

local function centre(rect)
  return rect.x + rect.w / 2, rect.y + rect.h / 2
end

-- The tiled window nearest to the focused one in a direction, across monitors.
local function neighbour(dir)
  local from = focused and (placed[focused] or (windows[focused] and windows[focused].rect))
  if not from then return nil end
  local fx, fy = centre(from)
  local dx, dy = DIRS[dir][1], DIRS[dir][2]
  local best, best_score = nil, math.huge
  for id, rect in pairs(placed) do
    if id ~= focused then
      local x, y = centre(rect)
      local along = (x - fx) * dx + (y - fy) * dy
      local across = math.abs((x - fx) * dy) + math.abs((y - fy) * dx)
      if along > 0 then
        local score = along + 2 * across
        if score < best_score then best, best_score = id, score end
      end
    end
  end
  return best
end

local function swap(a, b)
  local leaf_a, leaf_b
  for _, tree in pairs(trees) do
    leaf_a = leaf_a or find(tree, a)
    leaf_b = leaf_b or find(tree, b)
  end
  if leaf_a and leaf_b then
    leaf_a.window, leaf_b.window = b, a
  end
end

local actions = {}
for dir in pairs(DIRS) do
  actions["focus_" .. dir] = function()
    local target = neighbour(dir)
    if target then bones.publish("desktop/windows/focus", { id = windows[target].id }) end
  end
  actions["swap_" .. dir] = function()
    local target = neighbour(dir)
    if target and focused and tiled(focused) then
      swap(focused, target)
      apply()
      update_border()
    end
  end
end

function actions.toggle_float()
  if not focused or not windows[focused] then return end
  if tiled(focused) then
    remove(focused)
    floating[focused] = true
  elseif not insert(focused) then
    bones.info("no room to tile " .. windows[focused].title)
  end
  apply()
  update_border()
end

-- Events -------------------------------------------------------------------

local function load_monitors(list)
  monitors = {}
  for _, monitor in ipairs(list or {}) do
    monitors[key(monitor.id)] = monitor
  end
end

local function on_opened(window)
  local id = key(window.id)
  if window.minimized then return end
  local known = windows[id] ~= nil
  windows[id] = window
  if not known or not (tiled(id) or floating[id]) then
    insert(id)
    apply()
  end
end

local function on_gone(id)
  if remove(id) then apply() end
  windows[id] = nil
  floating[id] = nil
  if focused == id then
    focused = nil
    update_border()
  end
end

local handlers = {
  ["desktop/windows/opened"] = on_opened,
  ["desktop/windows/closed"] = function(event) on_gone(key(event.id)) end,
  ["desktop/windows/minimized"] = function(event)
    local id = key(event.id)
    if remove(id) then apply() end
    if windows[id] then windows[id].minimized = true end
    windows[id] = nil
    if focused == id then focused = nil; update_border() end
  end,
  ["desktop/windows/focused"] = function(event)
    focused = key(event.id)
    update_border()
  end,
  ["desktop/windows/moved"] = function(event)
    local id = key(event.id)
    if windows[id] then
      windows[id].rect = event.rect
      if id == focused and not placed[id] then update_border() end
    end
  end,
  ["desktop/monitors/changed"] = function(event)
    load_monitors(event.monitors)
    local ids = {}
    for id in pairs(windows) do
      if tiled(id) then table.insert(ids, id) end
    end
    trees = {}
    table.sort(ids)
    for _, id in ipairs(ids) do insert(id) end
    apply()
    update_border()
  end,
  ["desktop/hotkeys/pressed"] = function(event)
    if event.owner == bones.name and actions[event.id] then
      actions[event.id]()
    end
  end,
}

function on_message(topic, sender, payload)
  local handler = handlers[topic]
  if handler then handler(bones.json.decode(payload)) end
end

function init()
  local reply, err = bones.send(QUERY, "")
  if not reply then
    bones.warn("cannot reach the desktop module: " .. tostring(err))
    return
  end
  local snapshot = bones.json.decode(reply)
  if not snapshot.capabilities.windows then
    bones.warn("window management is not supported on " .. snapshot.platform .. "; the tiler does nothing here")
    return
  end
  load_monitors(snapshot.monitors)
  for topic in pairs(handlers) do bones.subscribe(topic) end
  for name, keys in pairs(setting("keys", {})) do
    if actions[name] then
      bones.publish("desktop/hotkeys/register", { id = name, keys = keys })
    else
      bones.warn("unknown action in keys: " .. name)
    end
  end
  local list = snapshot.windows or {}
  table.sort(list, function(a, b) return a.id < b.id end)
  for _, window in ipairs(list) do
    windows[key(window.id)] = window
  end
  focused = snapshot.focused and key(snapshot.focused) or nil
  for _, window in ipairs(list) do
    insert(key(window.id))
  end
  apply()
  update_border()
  bones.info("tiling " .. #list .. " windows")
end
