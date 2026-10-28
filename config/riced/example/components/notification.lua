-- Pure notification builder. Use as:
--   iced.notification({ icon = "bell", title = "Hello", body = "Text" })
iced.define("notification", function(props)
	props = props or {}

	local icon = props.icon
	if type(icon) == "string" then
		icon = icon ~= "" and iced.icon(icon) or nil
	end

	local title = props.title
	if title == nil then
		title = iced.text("")
	elseif type(title) ~= "table" then
		title = iced.text(title)
	end

	local header = {}
	if icon ~= nil then
		header[#header + 1] = icon
	end
	if title ~= nil then
		header[#header + 1] = title
	end
	if #header == 0 then
		header = iced.text("")
	else
		header = iced.row(header):spacing(6)
	end

	local body = props.body
	if body == nil then
		body = iced.space()
	elseif type(body) ~= "table" then
		body = iced.text(body)
	end
	local separator = props.body == nil and iced.space() or iced.separator()
	return iced.container(iced.column({
		header,
		separator,
		body,
	}))
end)
