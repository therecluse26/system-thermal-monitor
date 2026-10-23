# NixOS module for Thermite:
#
# - programs.thermite installs the window, from a release AppImage.
# - services.thermite-fand starts the fan daemon, thermite-fand, as root at boot. So one
#   user's fan settings run without the window and without a password dialog.
#
#   programs.thermite = {
#     enable = true;
#     appimage = inputs.thermite-appimage;
#   };
#   services.thermite-fand = {
#     enable = true;
#     user = "alice";
#   };
{
  config,
  lib,
  pkgs,
  utils,
  ...
}:

let
  window = config.programs.thermite;
  cfg = config.services.thermite-fand;
in
{
  options.programs.thermite = {
    enable = lib.mkEnableOption "the Thermite window, from a release AppImage";

    appimage = lib.mkOption {
      type = lib.types.path;
      example = lib.literalExpression "inputs.thermite-appimage";
      description = ''
        The release AppImage that the window runs. A flake input with `flake = false` of
        <https://github.com/therecluse26/thermite/releases/latest/download/thermite_amd64.AppImage>
        gets the newest release at each `nix flake update` of that input.
      '';
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./appimage.nix { inherit (window) appimage; };
      defaultText = lib.literalMD "the window, from {option}`programs.thermite.appimage`";
      description = "The Thermite window package.";
    };
  };

  options.services.thermite-fand = {
    enable = lib.mkEnableOption "the Thermite fan daemon, which runs one user's fan settings from boot";

    user = lib.mkOption {
      type = lib.types.passwdEntry lib.types.str;
      example = "alice";
      description = ''
        The user whose fan settings the daemon runs. The daemon reads
        {file}`~/.config/local.thermite/settings.json` of this user once per second.
        So a setting that the user applies in the window reaches the fans within a second.
      '';
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalMD "thermite-fand, built from this repository";
      description = "The thermite-fand package.";
    };
  };

  config = lib.mkMerge [
    (lib.mkIf window.enable {
      environment.systemPackages = [ window.package ];
    })

    (lib.mkIf cfg.enable {
      warnings = lib.optional (!config.users.users ? ${cfg.user}) ''
        services.thermite-fand.user is "${cfg.user}", but users.users has no user of that name.
        Without that user, the daemon stops at once, and systemd starts it again every 5 seconds.
      '';

      systemd.services.thermite-fand = {
        description = "Thermite fan daemon";
        wantedBy = [ "multi-user.target" ];
        # No `after` on other fan controllers. The NixOS fw-fanctrl unit starts after
        # multi-user.target, so that order is a cycle, and systemd then drops this start job.
        # The daemon finds another fan controller at its next check, within 30 seconds.
        serviceConfig = {
          ExecStart = utils.escapeSystemdExecArgs [
            (lib.getExe cfg.package)
            cfg.user
          ];
          # On SIGTERM the daemon gives every fan back to its automatic mode, then exits.
          Restart = "on-failure";
          RestartSec = 5;
          # The daemon writes /run/thermite/status.json, and the window reads it as the user.
          # systemd removes the directory when the service stops.
          RuntimeDirectory = "thermite";
          RuntimeDirectoryMode = "0755";
          UMask = "0022";

          # The daemon writes to /sys/class/hwmon. It reads /proc and the user's settings file.
          # So ProtectKernelTunables stays off: it makes /sys read-only.
          # CAP_DAC_READ_SEARCH lets root read the settings in a home directory of mode 700.
          CapabilityBoundingSet = [ "CAP_DAC_READ_SEARCH" ];
          NoNewPrivileges = true;
          ProtectSystem = "strict";
          ProtectHome = "read-only";
          PrivateTmp = true;
          PrivateDevices = true;
          PrivateNetwork = true;
          RestrictAddressFamilies = [ "AF_UNIX" ];
          ProtectClock = true;
          ProtectControlGroups = true;
          ProtectHostname = true;
          ProtectKernelLogs = true;
          ProtectKernelModules = true;
          LockPersonality = true;
          MemoryDenyWriteExecute = true;
          RestrictNamespaces = true;
          RestrictRealtime = true;
          RestrictSUIDSGID = true;
          SystemCallArchitectures = "native";
          SystemCallFilter = [
            "@system-service"
            "~@privileged"
            "~@resources"
          ];
        };
      };
    })
  ];
}
