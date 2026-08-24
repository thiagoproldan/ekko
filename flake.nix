{
  description = "Ekko dev environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            util-linux # flock(1): the storage tests spawn a real external lock holder
            rust-analyzer
            pkg-config # several crates (e.g. clipboard backends) probe system libs at build time
          ];

          # Written to stderr, not stdout: this shell may be entered non-
          # interactively via `nix develop --command ekko --json ...` from a
          # script/agent, and this banner must never land in --json output.
          shellHook = ''
            export RUST_SRC_PATH="${pkgs.rustPlatform.rustLibSrc}"
            echo "ekko devshell -- $(rustc --version)" >&2
          '';
        };
      });
}
