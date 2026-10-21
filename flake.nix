{
  description = "peat — agent memory as a fold";
  # Built packages are published to this cache by .github/workflows/nix-cache.yml,
  # so consumers substitute instead of compiling.
  nixConfig = {
    extra-substituters = [ "https://flowerornament.cachix.org" ];
    extra-trusted-public-keys = [
      "flowerornament.cachix.org-1:gSODgIXgfRANrEGITBOF8XWaEKNy8hkNGfRVwqUG46c="
    ];
  };
  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }:
    let
      peatVersion = "0.6.0";
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
