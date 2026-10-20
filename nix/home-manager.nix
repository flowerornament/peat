{ self }:
{ config, lib, pkgs, ... }:
let
  defaultPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
  isRelativeSkillTarget = target: target != "" && builtins.substring 0 1 target != "/";
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

    skill = {
      enable = lib.mkEnableOption "peat skill symlink management";

      targets = lib.mkOption {
        type = lib.types.listOf (lib.types.addCheck lib.types.str isRelativeSkillTarget);
        default = [ ".agents/skills/peat" ".claude/skills/peat" ];
        example = lib.literalExpression ''
          [
            ".agents/skills/peat"
            ".claude/skills/peat"
            ".claude-work/skills/peat"
          ]
        '';
        description = ''
          Home-relative directories where Home Manager should symlink the
          package's `share/peat/skills/peat` (the skill that matches the
          installed peat). Codex reads `~/.agents/skills`; Claude Code reads
          `skills/` in each config dir, so add one target per extra
          `CLAUDE_CONFIG_DIR` (e.g. `.claude-work/skills/peat`).
        '';
      };
    };
  };

  config =
    let
      cfg = config.programs.peat;
      hasUniqueSkillTargets = builtins.length cfg.skill.targets == builtins.length (lib.unique cfg.skill.targets);
      skillFiles = lib.genAttrs cfg.skill.targets (_: {
        source = "${cfg.package}/share/peat/skills/peat";
      });
    in
    lib.mkMerge [
      {
        assertions = [
          {
            assertion = !cfg.skill.enable || cfg.enable;
            message = "programs.peat.skill.enable requires programs.peat.enable = true";
          }
          {
            assertion = !cfg.skill.enable || hasUniqueSkillTargets;
            message = "programs.peat.skill.targets must not contain duplicate paths";
          }
        ];
      }
      (lib.mkIf cfg.enable {
        home.packages = [ cfg.package ];
      })
      (lib.mkIf (cfg.enable && cfg.skill.enable) {
        home.file = skillFiles;
      })
    ];
}
