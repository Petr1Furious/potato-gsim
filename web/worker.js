// The worker beside the page: it runs the page's own program (never started as a game) to
// work out the bodies' future and the ship's predicted paths, and talks to the page only in
// messages. See crates/gsim-client/src/worker.rs.
"use strict";
let exports = null;
const inbox = [];
// Going on with work without the delay a timer would add: a message to itself.
const again = new MessageChannel();
let pending = false;

const bytes = (ptr, len) => new Uint8Array(exports.memory.buffer, ptr, len);

function kick() {
    if (exports && !pending) {
        pending = true;
        again.port2.postMessage(0);
    }
}

again.port1.onmessage = function () {
    pending = false;
    for (const message of inbox.splice(0)) {
        bytes(exports.gsim_worker_inbox(message.length), message.length).set(message);
        exports.gsim_worker_message();
    }
    exports.gsim_worker_predict();
    // A few milliseconds of rows at a time, so that messages are not kept waiting.
    if (exports.gsim_worker_work(4)) kick();
};

onmessage = (event) => {
    inbox.push(new Uint8Array(event.data));
    kick();
};

(async function () {
    const module = await WebAssembly.compileStreaming(fetch("gsim-client.wasm"));
    // Everything the program could ask of a page does nothing here: the worker only ever
    // calls the parts that need none of it.
    const env = {};
    for (const item of WebAssembly.Module.imports(module)) {
        if (item.kind === "function") env[item.name] = () => 0;
    }
    env.gsim_now = () => performance.now() / 1000;
    // (The window library's clock, which the program's timing goes by.)
    env.now = () => Date.now() / 1000;
    env.gsim_worker_post = function (ptr, len) {
        const copy = bytes(ptr, len).slice();
        postMessage(copy.buffer, [copy.buffer]);
    };
    exports = (await WebAssembly.instantiate(module, { env })).exports;
    postMessage("ready");
    kick();
})().catch((error) => {
    console.error("the worker could not start:", error);
    postMessage("failed");
});
