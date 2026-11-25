-- The tiler: a dwindle layout per monitor (UC-TILE-01 to 05).
--
-- Each monitor holds a tree. A leaf is a tile: one window, or a stack of
-- windows of which one shows and the rest are minimized. A split divides its
-- area between two children, side by side ("h") or stacked ("v"), at a
-- ratio. A new window splits the focused tile along its longer side; one that
-- fits nowhere joins a stack. Everything happens through the desktop
-- module's bus topics.

local QUERY = "desktop/windows/query"

local windows = {}   -- id -> window info, with the real frame in .rect
local floating = {}  -- id -> true for windows outside the layout
local maximized = {} -- id -> true while the user has it maximized
local returns = {}   -- id -> a window of the stack it left by being enlarged
local retries = {}   -- id -> how often a window that ended elsewhere was moved again
local minimum = {}   -- id -> { w?, h? } a window refused to go below
local pending = {}   -- id -> true while a move we asked for has not settled
local hidden = {}    -- id -> true for stack members we minimized
local trees = {}     -- monitor id -> root node
local monitors = {}  -- monitor id -> monitor
local placed = {}    -- id -> rect the last layout gave it
local focused = nil
local drag = nil     -- { id, rect, target?, zone?, preview? } while the user drags
local shown = {}     -- overlay id -> true for overlays currently drawn

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
local BORDER = setting("border", {})
local FOCUS_COLOR = BORDER.color or "#00bcd4"
local FOCUS_WIDTH = BORDER.width or 3
local FLOAT_COLOR = BORDER.float_color or "#ffb300"
local DROP_FILL = BORDER.drop_fill or 0.25
local TAB_COLOR = BORDER.tab_color or "#7f8c8d"
local BADGE_COLOR = BORDER.badge_color or "#00838f"
local BADGE_SIZE = 28
-- What dropping a window on the middle of a tile does, and where a new window goes.
local DROP_MIDDLE = setting("drop_middle", "stack")
local NEW_WINDOW = setting("new_window", "split")
local FOCUS_FRAME = BORDER.enabled ~= false
local RADIUS = BORDER.radius or 0

-- Sizes --------------------------------------------------------------------

-- The smallest tile a window takes: the configured minimum, raised to what
-- the window itself refused to go below.
local function need(id)
  local own = minimum[id] or {}
  return { w = math.max(MIN_W, own.w or 0), h = math.max(MIN_H, own.h or 0) }
end

-- The smallest tile a leaf takes: what its largest member needs.
local function need_of_leaf(leaf)
  local size = need(leaf.window)
  for _, id in ipairs(leaf.stack or {}) do
    local own = need(id)
    size = { w = math.max(size.w, own.w), h = math.max(size.h, own.h) }
  end
  return size
end

-- The smallest area a subtree takes: its children side by side or stacked.
local function need_of(node)
  if node.window then return need_of_leaf(node) end
  local a, b = need_of(node.a), need_of(node.b)
  if node.split == "h" then
    return { w = a.w + GAP + b.w, h = math.max(a.h, b.h) }
  end
  return { w = math.max(a.w, b.w), h = a.h + GAP + b.h }
end

local function fits(id, rect)
  local size = need(id)
  return rect.w >= size.w and rect.h >= size.h
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

-- Fills `out` with each shown window's rect and remembers each node's own
-- rect, which a resize needs to turn an edge into a ratio.
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

local function holds(leaf, id)
  if leaf.window == id then return true end
  for _, member in ipairs(leaf.stack or {}) do
    if member == id then return true end
  end
  return false
end

-- The leaf holding `id`, shown or stacked, and its parent.
local function find(node, id, parent)
  if node.window then
    if holds(node, id) then return node, parent end
    return nil
  end
  local found, owner = find(node.a, id, node)
  if found then return found, owner end
  return find(node.b, id, node)
end

local function find_any(id)
  for monitor_id, tree in pairs(trees) do
    local leaf, parent = find(tree, id)
    if leaf then return leaf, parent, monitor_id end
  end
  return nil
end

-- The splits from the root down to the window's leaf, each with the side
-- ("a" or "b") the window is on.
local function path_to(node, id, path)
  path = path or {}
  if node.window then
    if holds(node, id) then return path end
    return nil
  end
  for _, side in ipairs({ "a", "b" }) do
    table.insert(path, { node = node, side = side })
    if path_to(node[side], id, path) then return path end
    table.remove(path)
  end
  return nil
end

local function leaves(node, out)
  out = out or {}
  if node.window then
    table.insert(out, node)
  else
    leaves(node.a, out)
    leaves(node.b, out)
  end
  return out
end

local function last_leaf(node)
  while not node.window do node = node.b end
  return node
end

local function same(a, b)
  return a and b and math.abs(a.x - b.x) <= SLACK and math.abs(a.y - b.y) <= SLACK
    and math.abs(a.w - b.w) <= SLACK and math.abs(a.h - b.h) <= SLACK
end

local function tiled(id)
  return find_any(id) ~= nil
end

-- Overlays -----------------------------------------------------------------

local function overlay(id, rect, color, width, want, fill, badge, attach)
  want[id] = true
  -- Tab strips are lines; everything else follows the configured corners.
  local radius = rect.h > 0 and RADIUS or 0
  bones.publish("desktop/overlay/set",
    { id = id, rect = rect, color = color, width = width, fill = fill, radius = radius,
      badge = badge, attach = attach })
end

-- The badge sits in the top left corner of the tile, tucked close to it.
local function badge_rect(rect)
  return { x = rect.x + 2, y = rect.y + 2, w = BADGE_SIZE, h = BADGE_SIZE }
end

-- Draws every overlay from the current state and clears the rest: the focus
-- border (unless disabled), an amber frame on each floating window, a tab strip over each stack,
-- and the drop preview while the user drags. The window being dragged gets no
-- frame: it would lag behind the cursor.
local function decorate()
  local want = {}
  do
    for id in pairs(floating) do
      local window = windows[id]
      if window and not maximized[id] and not hidden[id] and not (drag and drag.id == id) then
        overlay("float:" .. id, window.rect, FLOAT_COLOR, id == focused and FOCUS_WIDTH or 2, want)
      end
    end
    local window = focused and windows[focused]
    if FOCUS_FRAME and window and not floating[focused] and not hidden[focused]
      and not (drag and drag.id == focused) then
      overlay("focus", window.rect, FOCUS_COLOR, FOCUS_WIDTH, want)
    end
    for _, tree in pairs(trees) do
      for _, leaf in ipairs(leaves(tree)) do
        local rect = leaf.stack and #leaf.stack > 1 and placed[leaf.window]
        if rect then
          -- One segment per member, in the gap above the tile.
          local n = #leaf.stack
          local width = rect.w // n
          for i, member in ipairs(leaf.stack) do
            local segment = { x = rect.x + (i - 1) * width + 2, y = rect.y - 4, w = width - 4, h = 0 }
            overlay("tab:" .. leaf.stack[1] .. ":" .. i, segment,
              member == leaf.window and FOCUS_COLOR or TAB_COLOR, 2, want)
          end
          -- A badge in the corner counts the windows and opens the picker.
          overlay("badge:" .. leaf.stack[1], badge_rect(rect), BADGE_COLOR, 0, want, nil, tostring(n),
            windows[leaf.window] and windows[leaf.window].id)
        end
      end
    end
  end
  if drag and drag.preview then
    overlay("drop", drag.preview, FOCUS_COLOR, 4, want, DROP_FILL)
  end
  for id in pairs(shown) do
    if not want[id] then bones.publish("desktop/overlay/clear", { id = id }) end
  end
  shown = want
end

-- Moving -------------------------------------------------------------------

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
  decorate()
end

local function monitor_of(window)
  local id = key(window.monitor)
  if monitors[id] then return id end
  return next(monitors)
end

-- Puts a window that just left the layout in the middle of its monitor, at
-- its own size and in front, so it does not sit inside one tile.
local function centre_float(id)
  local window = windows[id]
  if not window or maximized[id] then return end
  local monitor = monitors[monitor_of(window)]
  if not monitor then return end
  local work, r = monitor.work, window.rect
  local w, h = math.min(r.w, work.w), math.min(r.h, work.h)
  bones.publish("desktop/windows/move", {
    id = window.id,
    rect = { x = work.x + (work.w - w) // 2, y = work.y + (work.h - h) // 2, w = w, h = h },
    raise = true,
  })
end

local function float(id, reason)
  floating[id] = true
  if reason then bones.info("floating " .. windows[id].title .. ": " .. reason) end
  centre_float(id)
end

-- Stacks -------------------------------------------------------------------

local picking = nil -- first member of the stack whose picker is open

-- Shows `id` in its stack: the member shown before is minimized.
local function show(leaf, id)
  local before = leaf.window
  if before == id then return end
  hidden[before] = true
  bones.publish("desktop/windows/minimize", { id = windows[before].id })
  if hidden[id] then
    hidden[id] = nil
    bones.publish("desktop/windows/restore", { id = windows[id].id })
  end
  leaf.window = id
end

-- Adds `id` to the stack of `leaf` and shows it there.
local function stack_onto(leaf, id)
  leaf.stack = leaf.stack or { leaf.window }
  table.insert(leaf.stack, id)
  floating[id] = nil
  show(leaf, id)
  bones.debug("stacked " .. windows[id].title .. " with " .. windows[leaf.stack[1]].title)
end

-- The tile a window that fits nowhere joins: the largest one it fits, else
-- the focused one, else the largest.
local function stack_target(id, monitor_id)
  local tree = trees[monitor_id]
  if not tree then return nil end
  local rects = {}
  layout(tree, area(monitors[monitor_id]), rects)
  local best, best_area, largest, largest_area = nil, -1, nil, -1
  for _, leaf in ipairs(leaves(tree)) do
    local rect = rects[leaf.window]
    local size = rect.w * rect.h
    if fits(id, rect) and size > best_area then best, best_area = leaf, size end
    if size > largest_area then largest, largest_area = leaf, size end
  end
  if best then return best end
  local focus_leaf = focused and find(tree, focused)
  return focus_leaf or largest
end

-- Tiling -------------------------------------------------------------------

-- Splits `leaf` in `dir`, putting the new window `id` first or second.
local function split_leaf(leaf, dir, id, first)
  local old = { window = leaf.window, stack = leaf.stack }
  local new = { window = id }
  leaf.window, leaf.stack, leaf.split, leaf.ratio = nil, nil, dir, 0.5
  if first then
    leaf.a, leaf.b = new, old
  else
    leaf.a, leaf.b = old, new
  end
end

-- Adds a window to its monitor's tree: a split of the focused tile when it
-- fits, a stack when it does not (UC-TILE-02). Returns whether it was tiled.
local function insert(id, fresh)
  local window = windows[id]
  local monitor_id = monitor_of(window)
  if not monitor_id then return false end
  local tree = trees[monitor_id]
  if not tree then
    trees[monitor_id] = { window = id }
    floating[id] = nil
    return true
  end
  if fresh and NEW_WINDOW == "stack" then
    -- A window that just opened joins the focused tile even where there is room.
    local focus_leaf = focused and focused ~= id and find(tree, focused)
    if focus_leaf then
      stack_onto(focus_leaf, id)
      return true
    end
  end
  local rects = {}
  layout(tree, area(monitors[monitor_id]), rects)
  -- The focused tile first, then the others from the largest down; a stack
  -- only when no tile can be split.
  local candidates = leaves(tree)
  table.sort(candidates, function(a, b)
    local ra, rb = rects[a.window], rects[b.window]
    return ra.w * ra.h > rb.w * rb.h
  end)
  local focus_leaf = focused and focused ~= id and find(tree, focused)
  if focus_leaf then table.insert(candidates, 1, focus_leaf) end
  for _, target in ipairs(candidates) do
    local rect = rects[target.window]
    local target_need = need_of_leaf(target)
    -- The longer side first; the other when the windows only fit that way.
    local dirs = rect.w >= rect.h and { "h", "v" } or { "v", "h" }
    for _, dir in ipairs(dirs) do
      local kept, new = split_rect(rect, dir, 0.5, target_need, need(id))
      if fits(id, new) and kept.w >= target_need.w and kept.h >= target_need.h then
        local span = (dir == "h" and rect.w or rect.h) - GAP
        split_leaf(target, dir, id, false)
        target.ratio = (dir == "h" and kept.w or kept.h) / span
        floating[id] = nil
        return true
      end
    end
  end
  stack_onto(stack_target(id, monitor_id), id)
  return true
end

-- Takes a window out of the tree. A stack shows its next member; a lone
-- window gives its space to its sibling.
local function remove(id)
  local leaf, parent, monitor_id = find_any(id)
  if not leaf then return false end
  if leaf.stack and #leaf.stack > 1 then
    for i, member in ipairs(leaf.stack) do
      if member == id then
        table.remove(leaf.stack, i)
        break
      end
    end
    if leaf.window == id then
      local next_id = leaf.stack[1]
      leaf.window = next_id
      if hidden[next_id] then
        hidden[next_id] = nil
        bones.publish("desktop/windows/restore", { id = windows[next_id].id })
      end
    end
    if #leaf.stack == 1 then leaf.stack = nil end
    return true
  end
  if not parent then
    trees[monitor_id] = nil
  else
    local sibling = parent.a == leaf and parent.b or parent.a
    parent.window, parent.stack, parent.split, parent.ratio =
      sibling.window, sibling.stack, sibling.split, sibling.ratio
    parent.a, parent.b = sibling.a, sibling.b
  end
  return true
end

-- Exchanges two tiles' contents, stacks included.
local function swap_leaves(a, b)
  a.window, b.window = b.window, a.window
  a.stack, b.stack = b.stack, a.stack
end

-- A tiled window came out larger than its tile: the app has a minimum size.
-- Remember it and make room when that squeezes nobody; otherwise move it onto
-- a stack, and float it when it refuses even there.
local function refused(id, rect)
  local tile = placed[id]
  local own = minimum[id] or {}
  minimum[id] = {
    w = rect.w > tile.w + SLACK and rect.w or own.w,
    h = rect.h > tile.h + SLACK and rect.h or own.h,
  }
  local leaf = find_any(id)
  local was_stacked = leaf and leaf.stack and #leaf.stack > 1
  apply()
  local room = true
  for other, r in pairs(placed) do
    if not fits(other, r) then room = false end
  end
  if room then
    bones.debug(windows[id].title .. " needs at least " .. rect.w .. "x" .. rect.h .. "; made room")
    return
  end
  remove(id)
  if was_stacked then
    float(id, "it needs at least " .. rect.w .. "x" .. rect.h)
  else
    local monitor_id = monitor_of(windows[id])
    local target = monitor_id and stack_target(id, monitor_id)
    if target then stack_onto(target, id) else float(id, "no tile for it") end
  end
  apply()
end

local function path_of(id)
  for _, tree in pairs(trees) do
    local path = path_to(tree, id)
    if path then return path end
  end
  return nil
end

-- The user dragged an edge of a tiled window: move the split that edge
-- belongs to.
local function resized(id, rect)
  local tile = placed[id]
  local path = path_of(id)
  if not (tile and path) then return end
  -- A move keeps the size: it snaps back without touching any split.
  if math.abs(rect.w - tile.w) <= SLACK and math.abs(rect.h - tile.h) <= SLACK then return end
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
  for _, edge in ipairs(edges) do
    if edge.moved then
      -- The nearest split along this axis with the window on that side.
      for i = #path, 1, -1 do
        local step = path[i]
        if step.node.split == edge.dir and step.side == edge.side then
          local r = step.node.rect
          local span = (edge.dir == "h" and r.w or r.h) - GAP
          step.node.ratio = math.min(0.9, math.max(0.1, edge.size(r) / span))
          break
        end
      end
    end
  end
end

-- Drag to place ------------------------------------------------------------

-- Where a window dropped at `cursor` would go: the tile under the cursor
-- (not the dragged window's own) and the zone within it. The middle third
-- stacks or swaps, as configured; elsewhere the nearest edge picks the side
-- of a split.
local function drop_target(id, cursor)
  -- A window shown in a stack may also split its own tile: it leaves the
  -- stack for the side of the tile its cursor is nearest to.
  local own = find_any(id)
  local stacked = own and own.stack and #own.stack > 1
  for other, rect in pairs(placed) do
    if (other ~= id or stacked) and cursor.x >= rect.x and cursor.x < rect.x + rect.w
      and cursor.y >= rect.y and cursor.y < rect.y + rect.h then
      local u, v = (cursor.x - rect.x) / rect.w, (cursor.y - rect.y) / rect.h
      if u > 1 / 3 and u < 2 / 3 and v > 1 / 3 and v < 2 / 3 then
        -- Its own tile's middle means no change.
        if other == id then return nil end
        return other, "middle", rect
      end
      local zone, distance = "left", u
      if 1 - u < distance then zone, distance = "right", 1 - u end
      if v < distance then zone, distance = "top", v end
      if 1 - v < distance then zone = "bottom" end
      local dir = (zone == "left" or zone == "right") and "h" or "v"
      local first, second = split_rect(rect, dir, 0.5)
      return other, zone, (zone == "left" or zone == "top") and first or second
    end
  end
  return nil
end

local function drop(id, target, zone)
  local own = find_any(id)
  if target == id then
    -- A stacked window dropped on an edge of its own tile: the tile splits,
    -- the window takes that side, and the rest of the stack keeps the other.
    if not (own and own.stack and #own.stack > 1) then return end
    remove(id)
    local dir = (zone == "left" or zone == "right") and "h" or "v"
    split_leaf(own, dir, id, zone == "left" or zone == "top")
    floating[id] = nil
    return
  end
  if zone == "middle" and (DROP_MIDDLE == "stack" or (own and own.stack and #own.stack > 1)) then
    -- Dropped on the middle of a tile, there being room or not: the window
    -- joins that tile's windows, and a stack it left loses one. Dragging out
    -- of a stack never swaps whole stacks.
    remove(id)
    local leaf = find_any(target)
    if leaf then stack_onto(leaf, id) end
    return
  end
  if zone == "middle" then
    local own, other = find_any(id), find_any(target)
    if own and other then
      swap_leaves(own, other)
    elseif other then
      -- A floating window takes the tile; the window there floats.
      other.window = id
      if other.stack then
        for i, member in ipairs(other.stack) do
          if member == target then other.stack[i] = id end
        end
      end
      floating[id] = nil
      float(target)
    end
    return
  end
  remove(id)
  local leaf = find_any(target)
  if not leaf then return end
  local dir = (zone == "left" or zone == "right") and "h" or "v"
  split_leaf(leaf, dir, id, zone == "left" or zone == "top")
  floating[id] = nil
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

local actions = {}
for dir in pairs(DIRS) do
  actions["focus_" .. dir] = function()
    local target = neighbour(dir)
    if target then bones.publish("desktop/windows/focus", { id = windows[target].id }) end
  end
  actions["swap_" .. dir] = function()
    local target = neighbour(dir)
    local a, b = focused and find_any(focused), target and find_any(target)
    if a and b then
      swap_leaves(a, b)
      apply()
    end
  end
end

function actions.toggle_float()
  if not focused or not windows[focused] then return end
  if tiled(focused) then
    remove(focused)
    float(focused)
  else
    -- Toggling back in is a request to try again, whatever it refused before.
    minimum[focused] = nil
    insert(focused)
  end
  apply()
end

-- A second key for the same toggle, which Caps Lock fits.
actions.toggle_float_alt = actions.toggle_float

-- Shows the next window of the focused tile's stack.
function actions.stack_next()
  local leaf = focused and find_any(focused)
  if not (leaf and leaf.stack and #leaf.stack > 1) then return end
  local index = 1
  for i, member in ipairs(leaf.stack) do
    if member == leaf.window then index = i end
  end
  local next_id = leaf.stack[index % #leaf.stack + 1]
  show(leaf, next_id)
  apply()
  bones.publish("desktop/windows/focus", { id = windows[next_id].id })
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
  windows[id], floating[id], maximized[id], pending[id], hidden[id] = nil, nil, nil, nil, nil
  returns[id], retries[id] = nil, nil
  if focused == id then focused = nil end
  if was_tiled then apply() else decorate() end
end

local function on_opened(window)
  local id = key(window.id)
  if window.minimized then return end
  local known = windows[id] ~= nil
  windows[id] = window
  if window.maximized then maximized[id] = true end
  local leaf = find_any(id)
  if leaf and hidden[id] then
    -- The user restored a hidden stack member: show it in its stack.
    hidden[id] = nil
    show(leaf, id)
    apply()
  elseif not known or not (leaf or floating[id] or returns[id]) then
    insert(id, true)
    apply()
  end
end

local function on_minimized(event)
  local id = key(event.id)
  -- Minimizing a stack member to hide it is ours, not the user's.
  if hidden[id] then return end
  forget(id)
end

local function on_moved(event)
  local id = key(event.id)
  local window = windows[id]
  if not window then return end
  window.rect = event.rect
  if id == focused or floating[id] then decorate() end
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
    elseif tiled(id) and not maximized[id] and not same(event.rect, tile) then
      -- It came out smaller than asked, as a window just brought back from
      -- the taskbar may: ask again, a couple of times at most, so a window
      -- that cannot take its tile does not make this loop.
      if (retries[id] or 0) < 2 then
        retries[id] = (retries[id] or 0) + 1
        pending[id] = true
        bones.publish("desktop/windows/move", { id = window.id, rect = tile })
      end
    else
      retries[id] = nil
    end
  end
  if id == focused or floating[id] then decorate() end
end

-- The badge was clicked: ask the user which window of the stack to show.
local function on_overlay_clicked(event)
  if event.owner ~= bones.name then return end
  local first = tostring(event.id):match("^badge:(.+)$")
  local leaf = first and find_any(first)
  local rect = leaf and placed[leaf.window]
  if not (leaf and leaf.stack and rect) then return end
  local items = {}
  for i, member in ipairs(leaf.stack) do
    local title = windows[member] and windows[member].title or tostring(member)
    if #title > 90 then title = title:sub(1, 89) .. "…" end
    table.insert(items, {
      id = i,
      label = title,
      checked = member == leaf.window,
      window = windows[member] and windows[member].id,
    })
  end
  picking = first
  local at = badge_rect(rect)
  bones.publish("desktop/overlay/menu", {
    id = "stack",
    items = items,
    at = { x = at.x, y = at.y + at.h + 4 },
    accent = FOCUS_COLOR,
  })
end

-- The user chose a window in the picker.
local function on_overlay_chosen(event)
  if event.owner ~= bones.name or event.id ~= "stack" then return end
  local first = picking
  picking = nil
  local leaf = first and find_any(first)
  local member = leaf and leaf.stack and event.item and leaf.stack[event.item]
  if not member then return end
  show(leaf, member)
  apply()
  bones.publish("desktop/windows/focus", { id = windows[member].id })
end

local function on_drag_started(event)
  local id = key(event.id)
  -- A floating window moves freely: only the toggle key tiles it, so a drag
  -- shows no drop preview and places it nowhere.
  if windows[id] and not maximized[id] and not floating[id] then
    -- A drag that starts on the window's own border is a resize; any other
    -- is a move. Decided once here: crossing to a monitor with another scale
    -- changes the window's size mid-move, which size alone cannot tell apart.
    local r, c = windows[id].rect, event.cursor
    local edge = 8
    local resizing = c and (c.x < r.x + edge or c.x >= r.x + r.w - edge
      or c.y < r.y + edge or c.y >= r.y + r.h - edge)
    drag = { id = id, rect = r, moving = not resizing }
  end
end

-- While the user moves a window (not resizes it), preview where it would go.
local function on_dragging(event)
  local id = key(event.id)
  if not drag or drag.id ~= id then return end
  windows[id].rect = event.rect
  local target, zone, preview
  if drag.moving then target, zone, preview = drop_target(id, event.cursor) end
  if not same(preview, drag.preview) or zone ~= drag.zone then
    drag.target, drag.zone, drag.preview = target, zone, preview
    decorate()
  end
end

local function on_dragged(event)
  local id = key(event.id)
  local window = windows[id]
  if not window then
    drag = nil
    return
  end
  window.rect = event.rect
  local current = drag
  drag = nil
  if maximized[id] then
    decorate()
    return
  end
  local leaf = find_any(id)
  local tile = placed[id]
  local cursor = event.cursor
  local left_tile = tile and cursor
    and (cursor.x < tile.x or cursor.x >= tile.x + tile.w or cursor.y < tile.y or cursor.y >= tile.y + tile.h)
  if current and current.id == id and current.target then
    drop(id, current.target, current.zone)
  elseif current and current.id == id and current.moving and left_tile
    and leaf and leaf.stack and #leaf.stack > 1 then
    -- Moved out of its stack onto no tile: it floats where it was dropped,
    -- and the stack loses one.
    remove(id)
    floating[id] = true
  elseif tiled(id) then
    -- A resize moves its split; a move without a target snaps back.
    resized(id, event.rect)
  end
  apply()
end

local handlers = {
  ["desktop/windows/opened"] = on_opened,
  ["desktop/windows/closed"] = function(event) forget(key(event.id)) end,
  ["desktop/windows/minimized"] = on_minimized,
  ["desktop/windows/focused"] = function(event)
    focused = key(event.id)
    decorate()
  end,
  ["desktop/windows/moved"] = on_moved,
  ["desktop/windows/settled"] = on_settled,
  ["desktop/overlay/clicked"] = on_overlay_clicked,
  ["desktop/overlay/chosen"] = on_overlay_chosen,
  ["desktop/windows/drag-started"] = on_drag_started,
  ["desktop/windows/dragging"] = on_dragging,
  ["desktop/windows/dragged"] = on_dragged,
  ["desktop/windows/maximized"] = function(event)
    local id = key(event.id)
    if not windows[id] then return end
    maximized[id] = event.maximized or nil
    if event.maximized then
      local leaf = find_any(id)
      if leaf and leaf.stack and #leaf.stack > 1 then
        -- Enlarged, a stacked window leaves its stack, which shows the next
        -- window; it remembers where it came from.
        remove(id)
        returns[id] = leaf.window
        apply()
      else
        decorate()
      end
    else
      -- Back to normal size, it rejoins the stack it left, if that is still
      -- there, and otherwise takes a place like any window.
      local anchor = returns[id]
      returns[id] = nil
      if anchor and not tiled(id) then
        local leaf = find_any(anchor)
        if leaf then stack_onto(leaf, id) else insert(id) end
      end
      apply()
    end
  end,
  ["desktop/monitors/changed"] = function(event)
    load_monitors(event.monitors)
    -- Layouts stay as they are, stacks included, and refit to their monitor.
    -- Only the windows of a monitor that went away move to another one, each
    -- stack together.
    local orphans = {}
    for monitor_id, tree in pairs(trees) do
      if not monitors[monitor_id] then
        for _, leaf in ipairs(leaves(tree)) do
          table.insert(orphans, leaf.stack and { table.unpack(leaf.stack) } or { leaf.window })
        end
        trees[monitor_id] = nil
      end
    end
    table.sort(orphans, function(a, b) return a[1] < b[1] end)
    for _, group in ipairs(orphans) do
      -- The window the stack showed goes first; the rest join it hidden.
      local shown = group[1]
      for _, member in ipairs(group) do
        if not hidden[member] then shown = member end
      end
      insert(shown)
      local leaf = find_any(shown)
      for _, member in ipairs(group) do
        if leaf and member ~= shown then
          leaf.stack = leaf.stack or { shown }
          table.insert(leaf.stack, member)
        end
      end
    end
    apply()
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
      if keys ~= "" then
        bones.publish("desktop/hotkeys/register", { id = name, keys = keys })
      end
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
  bones.info("tiling " .. #list .. " windows")
end

-- Stack members this instance hid would stay minimized after a reload or
-- stop; give them back.
function shutdown()
  for id in pairs(hidden) do
    if windows[id] then bones.publish("desktop/windows/restore", { id = windows[id].id }) end
  end
end
