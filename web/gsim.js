// What the game asks of the page beyond a canvas to draw on: a socket to the server,
// randomness, a clock, and somewhere to keep the player's settings and key.
"use strict";
(function () {
    const bytes = (ptr, len) => new Uint8Array(wasm_memory.buffer, ptr, len);
    const text = (ptr, len) => new TextDecoder().decode(bytes(ptr, len));

    // The one connection: 0 connecting, 1 open, 2 closed; and what has arrived on it.
    let socket = null, state = 2, inbox = [];
    // What `gsim_store_get` found, until `gsim_store_take` copies it out.
    let found = null;
    // Strings handed over by the interface library (it opens links through them).
    const strings = {};
    let next_string = 1;

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
