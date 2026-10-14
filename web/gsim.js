// What the game asks of the page beyond a canvas to draw on: a socket to the server,
// randomness, a clock, somewhere to keep the player's settings and key, and a worker beside
// the page (web/worker.js) to work out what lies ahead.
"use strict";
(function () {
    const bytes = (ptr, len) => new Uint8Array(wasm_memory.buffer, ptr, len);
    const text = (ptr, len) => new TextDecoder().decode(bytes(ptr, len));

    // The one connection: 0 connecting, 1 open, 2 closed; and what has arrived on it.
    let socket = null, state = 2, inbox = [];
    // What `gsim_store_get` found, until `gsim_store_take` copies it out.
    let found = null;
    // The worker: 0 while it loads, 1 once ready, 2 if it failed; and what it has sent.
    let worker = null, worker_state = 2, from_worker = [];
    // Strings handed over by the interface library (it opens links through them).
    const strings = {};
    let next_string = 1;

    // Drawing into floating-point images is something a browser only does once asked:
    // ask as soon as the window library gets its drawing context.
    const context = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (kind, ...options) {
        const gl = context.call(this, kind, ...options);
        if (gl && kind === "webgl2") gl.getExtension("EXT_color_buffer_float");
        return gl;
    };

    miniquad_add_plugin({
        name: "gsim",
        version: 1,
        register_plugin: function (imports) {
            const env = imports.env;

            env.gsim_ws_open = function (ptr, len) {
                // Nothing named: the server this page came from, next to the page.
                const named = text(ptr, len);
                const url = new URL(named === "" ? "ws" : named, location.href);
                url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
                const mine = new WebSocket(url);
                mine.binaryType = "arraybuffer";
                mine.onopen = () => { if (socket === mine) state = 1; };
                mine.onclose = mine.onerror = () => { if (socket === mine) state = 2; };
                mine.onmessage = (event) => { if (socket === mine) inbox.push(new Uint8Array(event.data)); };
                if (socket) socket.close();
                socket = mine;
                state = 0;
                inbox = [];
            };
            env.gsim_ws_state = () => state;
            env.gsim_ws_send = function (ptr, len) {
                // (A copy: the game's memory moves on.)
                if (state === 1) socket.send(bytes(ptr, len).slice());
            };
            env.gsim_ws_next = () => inbox.length > 0 ? inbox[0].length : -1;
            env.gsim_ws_take = function (ptr) {
                const message = inbox.shift();
                bytes(ptr, message.length).set(message);
            };
            env.gsim_ws_close = function () {
                if (socket) socket.close();
                socket = null;
                state = 2;
            };

            env.gsim_worker_start = function () {
                try {
                    worker = new Worker("worker.js");
                } catch (refused) {
                    return 0;
                }
                worker_state = 0;
                worker.onmessage = function (event) {
                    if (event.data === "ready") worker_state = 1;
                    else if (event.data === "failed") worker_state = 2;
                    else from_worker.push(new Uint8Array(event.data));
                };
                worker.onerror = () => { worker_state = 2; };
                return 1;
            };
            env.gsim_worker_state = () => worker_state;
            env.gsim_worker_send = function (ptr, len) {
                // (A copy, handed over whole: the game's memory moves on.)
                const copy = bytes(ptr, len).slice();
                if (worker) worker.postMessage(copy.buffer, [copy.buffer]);
            };
            env.gsim_worker_next = () => from_worker.length > 0 ? from_worker[0].length : -1;
            env.gsim_worker_take = function (ptr) {
                const message = from_worker.shift();
                bytes(ptr, message.length).set(message);
            };
            // (The worker's own way of answering: nothing to the page.)
            env.gsim_worker_post = () => {};

            env.gsim_copy = function (ptr, len) {
                // (Allowed only shortly after the player clicked, as they just did.)
                if (navigator.clipboard) navigator.clipboard.writeText(text(ptr, len)).catch(() => {});
            };
            env.gsim_now = () => performance.now() / 1000;
            env.gsim_random = (ptr, len) => { crypto.getRandomValues(bytes(ptr, len)); };

            env.gsim_store_get = function (ptr, len) {
                try {
                    const kept = localStorage.getItem("potato_gsim/" + text(ptr, len));
                    found = kept === null ? null : new TextEncoder().encode(kept);
                } catch (refused) {
                    found = null;
                }
                return found === null ? -1 : found.length;
            };
            env.gsim_store_take = (ptr) => { bytes(ptr, found.length).set(found); };
            env.gsim_store_set = function (name, name_len, ptr, len) {
                try {
                    localStorage.setItem("potato_gsim/" + text(name, name_len), text(ptr, len));
                } catch (refused) {
                    // A page that may keep nothing still plays.
                }
            };

            env.js_create_string = function (ptr, len) {
                strings[next_string] = text(ptr, len);
                return next_string++;
            };
            env.js_free_object = (id) => { delete strings[id]; };
            env.quad_url_link_open = (id, new_tab) => { window.open(strings[id], new_tab ? "_blank" : "_self"); };
        },
    });
})();
