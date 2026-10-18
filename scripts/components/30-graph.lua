-- Graph is a pure builder: it turns 0..100 series into an inert canvas.
-- The newest sample sits at the right edge; a partially filled window grows
-- in from the right instead of stretching. Series authors own the history.
iced.define("graph", function(props)
    props = props or {}
    local width = props.width or 200
    local height = props.height or 48
    local window = math.max(tonumber(props.window) or 60, 2)
    local pad = props.pad or 3
    local grid_color = props.grid_color or "#ffffff1a"
    local series = props.series or {}

    local rects = {}
    local lines = {}
    if props.background then
        rects[#rects + 1] = { x = 0, y = 0, width = width, height = height, color = props.background }
    end
    local rows = math.max(tonumber(props.grid_rows) or 4, 1)
    if props.grid ~= false then
        for i = 1, rows - 1 do
            local y = pad + (height - 2 * pad) * i / rows
            lines[#lines + 1] = {
                points = { { x = 0, y = y }, { x = width, y = y } },
                color = grid_color,
                width = 1,
            }
        end
    end

    local plot = height - 2 * pad
    local step = width / (window - 1)
    for _, s in ipairs(series) do
        local values = s.values or {}
        local n = #values
        if n >= 2 then
            local count = math.min(n, window)
            local first = n - count + 1
            local points = {}
            for i = first, n do
                local value = values[i] or 0
                if value < 0 then value = 0 elseif value > 100 then value = 100 end
                points[#points + 1] = {
                    x = width - (n - i) * step,
                    y = height - pad - value / 100 * plot,
                }
            end
            lines[#lines + 1] = {
                points = points,
                color = s.color or "#ffffff",
                width = s.width or 1.5,
            }
        end
    end

    return iced.canvas({ lines = lines, rects = rects }):width(width):height(height)
end)
