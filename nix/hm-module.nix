# Home Manager module for lazygitrs.
# `self` is the flake, used to resolve the default package for the host system.
self:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.lazygitrs;
  yamlFormat = pkgs.formats.yaml { };
in
{
  options.programs.lazygitrs = {
    enable = lib.mkEnableOption "lazygitrs, a faster memory-safe TUI git client";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.lazygitrs;
      defaultText = lib.literalExpression "lazygitrs.packages.\${system}.lazygitrs";
      description = "The lazygitrs package to install.";
    };

    settings = lib.mkOption {
      type = yamlFormat.type;
      default = { };
      example = lib.literalExpression ''
        {
          gui.theme.activeBorderColor = [ "green" "bold" ];
          git.autoRefresh = true;
        }
      '';
      description = ''
        Configuration written verbatim (as YAML) to
        {file}`$XDG_CONFIG_HOME/lazygitrs/config.yml`. lazygitrs reads the
        lazygit-compatible config schema; see its documentation for the
        available keys. Leave empty to manage the config file yourself.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    # Only write the config file when the user actually set something, so a
    # bare `enable = true;` does not clobber a hand-managed config.yml.
    xdg.configFile."lazygitrs/config.yml" = lib.mkIf (cfg.settings != { }) {
      source = yamlFormat.generate "lazygitrs-config.yml" cfg.settings;
    };
  };
}
