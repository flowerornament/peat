{
  description = "peat — agent memory as a fold";
  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }:
    let
      peatVersion = "0.1.0";
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f {
        pkgs = nixpkgs.legacyPackages.${system};
      });
    in {
      packages = forAllSystems ({ pkgs }: {
        default = import ./nix/package.nix {
          inherit pkgs;
          src = ./.;
          version = peatVersion;
        };
      });

      apps = forAllSystems ({ pkgs }: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/peat";
        };
      });

      homeManagerModules.default = import ./nix/home-manager.nix { inherit self; };
    };
}
