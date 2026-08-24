{
  description = "Taskbook dev environment";

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
            nodejs # provides node + npm
            xsel # clipboard backend used by clipboardy on Linux (--copy flag)
          ];

          # Written to stderr, not stdout: `nix develop --command tb --json ...`
          # is a realistic way to invoke this from a script/agent, and this
          # banner must never end up mixed into --json's stdout output.
          shellHook = ''
            echo "taskbook devshell — node $(node --version), npm $(npm --version)" >&2
            if [ ! -d node_modules ]; then
              echo "-> run 'npm install' to install dependencies" >&2
            fi
          '';
        };
      });
}
