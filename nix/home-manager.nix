{ self }:
{ config, lib, pkgs, ... }:
let
  defaultPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
in
{
  options.programs.peat = {
    enable = lib.mkEnableOption "peat agent memory";

    package = lib.mkOption {
      type = lib.types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "peat package from this flake";
      description = "The peat package to install.";
    };
  };

  config = lib.mkIf config.programs.peat.enable {
    home.packages = [ config.programs.peat.package ];
  };
}
