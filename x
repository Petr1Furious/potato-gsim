#!/bin/sh
# Run a command inside a shell that has the Rust toolchain (NixOS).
# Usage: ./x cargo test --workspace
exec nix shell nixpkgs#rustc nixpkgs#cargo nixpkgs#clippy nixpkgs#gcc -c "$@"
