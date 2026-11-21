# maat-wasm

The browser demo's bridge to the [Ma'at](https://github.com/IsomorphicAlgo/Ma-at) contact
scheduler. It compiles `maat-ground` to WebAssembly and exports three functions to JavaScript:

- `plan(requestJson)` builds a contact plan and returns it as JSON;
- `catalog(text)` lists the satellites in a catalog;
- `version()` returns the scheduler's version.

It adds no scheduling logic. The page in [`demo/`](../../demo) calls it; see the repository
README for how to build and open the demo.

Not published to crates.io: it is a front end, not a library.

License: MIT.
