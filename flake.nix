{
  description = "Ekko -- tasks, boards & notes for the command-line habitat";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Read rather than repeated: a version written in two places is one
        # that eventually disagrees with itself, the same way the declared
        # MSRV did before CI started reading it out of Cargo.toml.
        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = cargoToml.package.name;
          inherit (cargoToml.package) version;

          # `target/` is a few hundred megabytes of build output and would
          # otherwise be copied into the store on every evaluation.
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: _type: baseNameOf path != "target";
          };

          cargoLock.lockFile = ./Cargo.lock;

          # flock(1), for the storage tests that spawn a real external lock
          # holder. Check-time only -- the binary itself needs nothing.
          nativeCheckInputs = [ pkgs.util-linux ];

          meta = with pkgs.lib; {
            inherit (cargoToml.package) description;
            homepage = cargoToml.package.repository;
            license = licenses.mit;
            mainProgram = "ekko";
            # storage.rs reads /proc/<pid>/stat with no cfg fallback.
            platforms = platforms.linux;
          };
        };

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
