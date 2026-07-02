{
  description = "lazygitrs — a faster, memory-safe, more ergonomic TUI reimagining of lazygit";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems =
        f:
        lib.genAttrs systems (
          system:
          f {
            inherit system;
            pkgs = nixpkgs.legacyPackages.${system};
          }
        );
    in
    {
      packages = forAllSystems (
        { pkgs, ... }:
        let
          lazygitrs = pkgs.callPackage ./nix/package.nix { };
        in
        {
          inherit lazygitrs;
          default = lazygitrs;
        }
      );

      # Add `lazygitrs` to any nixpkgs instance.
      overlays.default = final: _prev: {
        lazygitrs = final.callPackage ./nix/package.nix { };
      };

      # Home Manager module: `programs.lazygitrs = { enable = true; settings = { ... }; };`
      homeManagerModules = rec {
        lazygitrs = import ./nix/hm-module.nix self;
        default = lazygitrs;
      };

      apps = forAllSystems (
        { system, ... }:
        {
          default = {
            type = "app";
            program = lib.getExe self.packages.${system}.lazygitrs;
            meta.description = "Run lazygitrs";
          };
        }
      );

      devShells = forAllSystems (
        { pkgs, system }:
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.lazygitrs ];
            packages = with pkgs; [
              rustc
              cargo
              clippy
              rustfmt
              rust-analyzer
              git
            ];
          };
        }
      );

      formatter = forAllSystems ({ pkgs, ... }: pkgs.nixfmt);
    };
}
