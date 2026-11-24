-- The tiler: a dwindle layout per monitor (UC-TILE-01 to 05).
--
-- Each monitor holds a tree. A leaf is one window; a split divides its area
-- between two children, side by side ("h") or stacked ("v"), at a ratio. A
-- new window splits the focused tile along its longer side. Everything
-- happens through the desktop module's bus topics.

local QUERY = "desktop/windows/query"

local windows = {}   -- id -> window info, with the real frame in .rect
local floating = {}  -- id -> true for windows left at their own size
local maximized = {} -- id -> true while the user has it maximized
local minimum = {}   -- id -> { w, h } a window refused to go below
local pending = {}   -- id -> true while a move we asked for has not settled
local trees = {}     -- monitor id -> root node
local monitors = {}  -- monitor id -> monitor
local placed = {}    -- id -> rect the last layout gave it
local focused = nil

-- How far a frame may differ from its tile before it counts as different.
local SLACK = 4

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

-- Sizes --------------------------------------------------------------------

-- The smallest tile a window takes: the configured minimum, raised to what
-- the window itself refused to go below.
local function need(id)
  local own = minimum[id] or {}
  return { w = math.max(MIN_W, own.w or 0), h = math.max(MIN_H, own.h or 0) }
end

-- The smallest area a subtree takes: its children side by side or stacked.
local function need_of(node)
  if node.window then return need(node.window) end
  local a, b = need_of(node.a), need_of(node.b)
  if node.split == "h" then
    return { w = a.w + GAP + b.w, h = math.max(a.h, b.h) }
  end
  return { w = math.max(a.w, b.w), h = a.h + GAP + b.h }
end

-- Layout -------------------------------------------------------------------

-- Divides `rect` at `ratio`, moved as little as needed for each side to get
-- at least `need_a` and `need_b` along the split, when both can.
local function split_rect(rect, dir, ratio, need_a, need_b)
  ratio = ratio or 0.5
  local span = (dir == "h" and rect.w or rect.h) - GAP
  local first = math.floor(span * ratio)
  if need_a and need_b then
    local lo = dir == "h" and need_a.w or need_a.h
    local hi = span - (dir == "h" and need_b.w or need_b.h)
    if lo <= hi then first = math.min(math.max(first, lo), hi) end
  end
  if dir == "h" then
    return { x = rect.x, y = rect.y, w = first, h = rect.h },
           { x = rect.x + first + GAP, y = rect.y, w = rect.w - first - GAP, h = rect.h }
  end
  return { x = rect.x, y = rect.y, w = rect.w, h = first },
         { x = rect.x, y = rect.y + first + GAP, w = rect.w, h = rect.h - first - GAP }
end

-- Fills `out` with each window's rect and remembers each node's own rect,
-- which a resize needs to turn an edge into a ratio.
local function layout(node, rect, out)
  node.rect = rect
  if node.window then
    out[node.window] = rect
    return
  end
  local a, b = split_rect(rect, node.split, node.ratio, need_of(node.a), need_of(node.b))
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

-- The splits from the root down to the window's leaf, each with the side
-- ("a" or "b") the window is on.
local function path_to(node, id, path)
  path = path or {}
  if node.window then
    if node.window == id then return path end
    return nil
  end
  for _, side in ipairs({ "a", "b" }) do
    table.insert(path, { node = node, side = side })
    if path_to(node[side], id, path) then return path end
    table.remove(path)
  end
  return nil
end

local function last_leaf(node)
  while not node.window do node = node.b end
  return node
end

local function same(a, b)
  return a and b and math.abs(a.x - b.x) <= SLACK and math.abs(a.y - b.y) <= SLACK
    and math.abs(a.w - b.w) <= SLACK and math.abs(a.h - b.h) <= SLACK
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
    -- A maximized window stays maximized; one already in place stays put,
    -- unless a move is still on its way and its known frame is stale.
    if window and not maximized[id] and (pending[id] or not same(window.rect, rect)) then
      pending[id] = true
      bones.publish("desktop/windows/move", { id = window.id, rect = rect })
    end
  end
end

-- Focus border -------------------------------------------------------------

-- Drawn around the window's real frame, which differs from its tile while
-- the window is maximized or still moving.
local function update_border()
  if not BORDER.enabled then return end
  local window = focused and windows[focused]
  if not window then
    bones.publish("desktop/overlay/clear", { id = "focus" })
    return
  end
  bones.publish("desktop/overlay/set", {
    id = "focus", rect = window.rect, color = BORDER.color, width = BORDER.width,
  })
end

-- Tiling -------------------------------------------------------------------

local function monitor_of(window)
  local id = key(window.monitor)
  if monitors[id] then return id end
  return next(monitors)
end

-- Whether `id` fits `rect`: the configured minimum, and the size the window
-- itself refused to go below, if it ever did.
local function fits(id, rect)
  local size = need(id)
  return rect.w >= size.w and rect.h >= size.h
end

-- Adds a window to its monitor's tree, or floats it when its tile would be
-- smaller than the minimum (UC-TILE-02). Returns whether it was tiled.
local function insert(id)
  local window = windows[id]
  local monitor_id = monitor_of(window)
  if not monitor_id then return false end
  local tree = trees[monitor_id]
  if not tree then
    if not fits(id, area(monitors[monitor_id])) then
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
  -- The longer side first; the other when the windows only fit that way.
  local dirs = rect.w >= rect.h and { "h", "v" } or { "v", "h" }
  for _, dir in ipairs(dirs) do
    local kept, new = split_rect(rect, dir, 0.5, need(target.window), need(id))
    if fits(id, new) and fits(target.window, kept) then
      local span = (dir == "h" and rect.w or rect.h) - GAP
      target.split = dir
      target.ratio = (dir == "h" and kept.w or kept.h) / span
      target.a, target.b = { window = target.window }, { window = id }
      target.window = nil
      floating[id] = nil
      return true
    end
  end
  floating[id] = true
  bones.debug("floating " .. window.title .. ": no tile fits it")
  return false
end

local function remove(id)
  for monitor_id, tree in pairs(trees) do
    local leaf, parent = find(tree, id)
    if leaf then
      if not parent then
        trees[monitor_id] = nil
      else
        local sibling = parent.a == leaf and parent.b or parent.a
        parent.window, parent.split, parent.ratio = sibling.window, sibling.split, sibling.ratio
        parent.a, parent.b = sibling.a, sibling.b
      end
      return true
    end
  end
  return false
end

-- Puts a window that just left the layout in the middle of its monitor, at
-- its own size, so it does not sit on top of one tile.
local function centre_float(id)
  local window = windows[id]
  local monitor = window and monitors[key(window.monitor)] or (next(monitors) and monitors[next(monitors)])
  if not window or not monitor or maximized[id] then return end
  local work, r = monitor.work, window.rect
  local w, h = math.min(r.w, work.w), math.min(r.h, work.h)
  bones.publish("desktop/windows/move", {
    id = window.id,
    rect = { x = work.x + (work.w - w) // 2, y = work.y + (work.h - h) // 2, w = w, h = h },
    raise = true,
  })
end

local function tiled(id)
  for _, tree in pairs(trees) do
    if find(tree, id) then return true end
  end
  return false
end

local function path_of(id)
  for _, tree in pairs(trees) do
    local path = path_to(tree, id)
    if path then return path end
  end
  return nil
end

-- A tiled window came out larger than its tile: the app has a minimum size.
-- Remember it, float the window, and give its space to the others.
local function refused(id, rect)
  local tile = placed[id]
  local own = minimum[id] or {}
  minimum[id] = {
    w = rect.w > tile.w + SLACK and rect.w or own.w,
    h = rect.h > tile.h + SLACK and rect.h or own.h,
  }
  apply()
  -- Room only counts when no other window was squeezed below its own need.
  local room = true
  for other, rect in pairs(placed) do
    if not fits(other, rect) then room = false end
  end
  if room then
    bones.debug(windows[id].title .. " needs at least " .. rect.w .. "x" .. rect.h .. "; made room")
    return
  end
  remove(id)
  floating[id] = true
  bones.info("floating " .. windows[id].title .. ": it needs at least " .. rect.w .. "x" .. rect.h)
  apply()
  centre_float(id)
end

-- The user dragged an edge of a tiled window: move the split that edge
-- belongs to. Any other drag snaps the window back into its tile.
local function resized(id, rect)
  local tile = placed[id]
  local path = path_of(id)
  if tile and path then
    local edges = {
      { dir = "h", side = "a", moved = math.abs((rect.x + rect.w) - (tile.x + tile.w)) > SLACK,
        size = function(r) return rect.x + rect.w - r.x end },
      { dir = "h", side = "b", moved = math.abs(rect.x - tile.x) > SLACK,
        size = function(r) return rect.x - GAP - r.x end },
      { dir = "v", side = "a", moved = math.abs((rect.y + rect.h) - (tile.y + tile.h)) > SLACK,
        size = function(r) return rect.y + rect.h - r.y end },
      { dir = "v", side = "b", moved = math.abs(rect.y - tile.y) > SLACK,
        size = function(r) return rect.y - GAP - r.y end },
    }
    -- A move keeps the size: snap back without touching any split.
    local resizing = math.abs(rect.w - tile.w) > SLACK or math.abs(rect.h - tile.h) > SLACK
    if resizing then
      for _, edge in ipairs(edges) do
        if edge.moved then
          -- The nearest split along this axis with the window on that side.
          for i = #path, 1, -1 do
            local step = path[i]
            if step.node.split == edge.dir and step.side == edge.side then
              local r = step.node.rect
              local span = (edge.dir == "h" and r.w or r.h) - GAP
              local ratio = edge.size(r) / span
              step.node.ratio = math.min(0.9, math.max(0.1, ratio))
              break
            end
          end
        end
      end
    end
  end
  apply()
  update_border()
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
    centre_float(focused)
  else
    -- Toggling back in is a request to try again, whatever it refused before.
    minimum[focused] = nil
    if not insert(focused) then
      bones.info("no room to tile " .. windows[focused].title)
    end
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

local function forget(id)
  local was_tiled = remove(id)
  windows[id], floating[id], maximized[id], pending[id] = nil, nil, nil, nil
  if was_tiled then apply() end
  if focused == id then
    focused = nil
    update_border()
  end
end

local function on_opened(window)
  local id = key(window.id)
  if window.minimized then return end
  local known = windows[id] ~= nil
  windows[id] = window
  if window.maximized then maximized[id] = true end
  if not known or not (tiled(id) or floating[id]) then
    insert(id)
    apply()
  end
end

local function on_moved(event)
  local id = key(event.id)
  local window = windows[id]
  if not window then return end
  window.rect = event.rect
  if id == focused then update_border() end
end

-- A move we asked for settled. It answers the current layout only when it
-- was a move to the tile the window has now; older moves are history.
local function on_settled(event)
  local id = key(event.id)
  local window = windows[id]
  if not window then return end
  window.rect = event.rect
  local tile = placed[id]
  if tile and same(event.requested, tile) then
    pending[id] = nil
    if tiled(id) and not maximized[id]
      and (event.rect.w > tile.w + SLACK or event.rect.h > tile.h + SLACK) then
      refused(id, event.rect)
    end
  end
  if id == focused then update_border() end
end

local handlers = {
  ["desktop/windows/opened"] = on_opened,
  ["desktop/windows/closed"] = function(event) forget(key(event.id)) end,
  ["desktop/windows/minimized"] = function(event) forget(key(event.id)) end,
  ["desktop/windows/focused"] = function(event)
    focused = key(event.id)
    update_border()
  end,
  ["desktop/windows/moved"] = on_moved,
  ["desktop/windows/settled"] = on_settled,
  ["desktop/windows/dragged"] = function(event)
    local id = key(event.id)
    if windows[id] then windows[id].rect = event.rect end
    if tiled(id) and not maximized[id] then resized(id, event.rect) end
  end,
  ["desktop/windows/maximized"] = function(event)
    local id = key(event.id)
    if not windows[id] then return end
    maximized[id] = event.maximized or nil
    if not event.maximized then apply() end
    update_border()
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
    if window.maximized then maximized[key(window.id)] = true end
  end
  focused = snapshot.focused and key(snapshot.focused) or nil
  for _, window in ipairs(list) do
    insert(key(window.id))
  end
  apply()
  update_border()
  bones.info("tiling " .. #list .. " windows")
end
