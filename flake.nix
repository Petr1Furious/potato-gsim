{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        # Loaded at run time by the Rust client's windowing layer (Linux only).
        clientLibs = pkgs.lib.optionals pkgs.stdenv.isLinux (
          with pkgs;
          [
            libx11
            libxi
            libxcursor
            libxrandr
            libxkbcommon
            libGL
          ]
        );
      in
      {
        devShells.default = pkgs.mkShell {
          packages =
            with pkgs;
            [
              # Rust rewrite (crates/)
              rustc
              cargo
              clippy
              rustfmt
            ]
            ++ clientLibs;
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath clientLibs;
        };
      }
    );
}
