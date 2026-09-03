-- The extension interface, built on top of the raw functions Atoll provides. Runs before the
-- extension itself and before the state is sandboxed.
local native = ...

local atoll = {}
local handlers = {}
local commands = {}
local workers = {}

atoll.id = native.id
atoll.version = native.version

local function join(...)
	local parts = {}
	for index = 1, select("#", ...) do
		parts[index] = tostring((select(index, ...)))
	end
	return table.concat(parts, " ")
end

-- Hands the work to Atoll and waits for the answer without blocking the window.
local function await(pending)
	local ok, value = coroutine.yield({ __atoll_pending = pending })
	if not ok then
		error(value, 0)
	end
	return value
end

function atoll.log(...)
	native.log("info", join(...))
end

function atoll.warn(...)
	native.log("warn", join(...))
end

function atoll.error(...)
	native.log("error", join(...))
end

function atoll.on(event, handler)
	assert(type(event) == "string", "atoll.on needs the name of an event")
	assert(type(handler) == "function", "atoll.on needs a function")
	local list = handlers[event]
	if not list then
		list = {}
		handlers[event] = list
	end
	table.insert(list, handler)
	return function()
		atoll.off(event, handler)
	end
end

function atoll.off(event, handler)
	local list = handlers[event]
	if not list then
		return
	end
	for index, candidate in ipairs(list) do
		if candidate == handler then
			table.remove(list, index)
			return
		end
	end
end

function atoll.sleep(milliseconds)
	return await(native.sleep(milliseconds))
end

atoll.ui = {
	toast = function(message)
		native.toast(tostring(message))
	end,
}

atoll.storage = {
	get = function(key)
		return native.storage_get(tostring(key))
	end,
	set = function(key, value)
		native.storage_set(tostring(key), value)
	end,
	remove = function(key)
		native.storage_set(tostring(key), nil)
	end,
	keys = function()
		return native.storage_keys()
	end,
}

atoll.tabs = {
	list = function()
		return native.tabs()
	end,
	current = function()
		for _, tab in ipairs(native.tabs()) do
			if tab.active then
				return tab
			end
		end
		return nil
	end,
	open = function(url, options)
		local background = false
		if type(options) == "table" and options.background then
			background = true
		end
		return native.open_tab(tostring(url), background)
	end,
	close = function(tab)
		native.close_tab(tab)
	end,
	activate = function(tab)
		native.activate_tab(tab)
	end,
}

atoll.commands = {
	register = function(command)
		assert(type(command) == "table", "atoll.commands.register needs a table")
		assert(type(command.id) == "string", "a command needs an id")
		assert(type(command.run) == "function", "a command needs run")
		commands[command.id] = command.run
		native.register_command(command.id, tostring(command.title or command.id), command.shortcut)
	end,
	unregister = function(id)
		commands[id] = nil
		native.unregister_command(tostring(id))
	end,
}

atoll.page = {
	url = function()
		return native.page_url()
	end,
	eval = function(code, options)
		local world = if type(options) == "table" then options.world else nil
		return await(native.evaluate(tostring(code), world))
	end,
	injectJs = function(code, options)
		local world = if type(options) == "table" then options.world else nil
		return await(native.evaluate(tostring(code), world))
	end,
	injectCss = function(css, options)
		local world = if type(options) == "table" then options.world else nil
		return await(native.inject_css(tostring(css), world))
	end,
}

atoll.content = {
	register = function(script)
		assert(type(script) == "table", "atoll.content.register needs a table")
		assert(type(script.id) == "string", "a page script needs an id")
		native.content_register(
			script.id,
			script.matches or {},
			script.css or "",
			script.js or "",
			script.world,
			script.runAt
		)
	end,
	unregister = function(id)
		native.content_unregister(tostring(id))
	end,
}

atoll.requests = {
	block = function(rule)
		assert(type(rule) == "table", "atoll.requests.block needs a table")
		native.request_rule(tostring(rule.id or "block"), rule.patterns or {}, rule.types or {}, nil)
	end,
	redirect = function(rule)
		assert(type(rule) == "table", "atoll.requests.redirect needs a table")
		assert(type(rule.to) == "string", "a redirect needs a target under \"to\"")
		native.request_rule(tostring(rule.id or "redirect"), rule.patterns or {}, rule.types or {}, rule.to)
	end,
	clear = function(id)
		native.clear_request_rules(if id ~= nil then tostring(id) else nil)
	end,
}

atoll.panels = {
	show = function(id)
		native.show_panel(tostring(id))
	end,
	hide = function(id)
		native.hide_panel(tostring(id))
	end,
	post = function(id, channel, payload)
		native.post_to_panel(tostring(id), tostring(channel), payload)
	end,
}

local worker = {}
worker.__index = worker

function worker:call(name, args)
	return await(native.worker_call(self.id, tostring(name), args))
end

function worker:stop()
	workers[self.id] = nil
	native.worker_stop(self.id)
end

atoll.worker = {
	spawn = function(source, calls)
		assert(type(source) == "string", "atoll.worker.spawn needs the source of the worker")
		local id = native.worker_spawn(source)
		workers[id] = if type(calls) == "table" then calls else {}
		return setmetatable({ id = id }, worker)
	end,
}

local function fetch(method, url, options)
	options = if type(options) == "table" then options else {}
	return await(native.fetch(method, tostring(url), options.headers, options.body))
end

atoll.http = {
	request = fetch,
	get = function(url, options)
		return fetch("GET", url, options)
	end,
	post = function(url, options)
		return fetch("POST", url, options)
	end,
}

-- What Atoll calls into. Handlers run one after another, an error in one does not stop the rest.
local dispatch = {}

function dispatch.event(event, payload)
	local list = handlers[event]
	if not list then
		return
	end
	for _, handler in ipairs(table.clone(list)) do
		local ok, problem = pcall(handler, payload)
		if not ok then
			native.report(tostring(problem))
		end
	end
end

-- A page called window.atoll.send. The first handler that returns something answers it.
function dispatch.message(channel, payload, url)
	local list = handlers["page.message"]
	if not list then
		return nil
	end
	for _, handler in ipairs(table.clone(list)) do
		local ok, result = pcall(handler, { channel = channel, payload = payload, url = url })
		if not ok then
			native.report(tostring(result))
		elseif result ~= nil then
			return result
		end
	end
	return nil
end

-- A worker called host.call. What its handler returns goes back into the worker.
function dispatch.worker(id, name, payload)
	local list = workers[id]
	local handler = if list then list[name] else nil
	if not handler then
		error(`this worker may not call "{name}", the extension does not know it`, 0)
	end
	return handler(payload)
end

function dispatch.command(id)
	local run = commands[id]
	if not run then
		return
	end
	local ok, problem = pcall(run)
	if not ok then
		native.report(tostring(problem))
	end
end

return atoll, dispatch
