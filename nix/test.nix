# NixOS VM test of the fan daemon's module: nix build .#checks.x86_64-linux.module
#
# The VM has no fans, so this tests the service around the daemon, not the fan control:
# the status file that the window reads, the settings in a home directory of mode 700,
# giving way to fw-fanctrl, and the stop. The hardening applies to each step.
{
  coreutils,
  testers,
  writeShellScriptBin,
}:

let
  # In /proc this looks like `fw-fanctrl run`, which is what the daemon looks for.
  fwFanctrl = writeShellScriptBin "fw-fanctrl" "${coreutils}/bin/sleep infinity";
in
testers.runNixOSTest {
  name = "thermite-fand";

  nodes.machine = {
    imports = [ ./module.nix ];
    users.users.alice.isNormalUser = true;
    services.thermite-fand = {
      enable = true;
      user = "alice";
    };
  };

  testScript = ''
    import json

    settings = "/home/alice/.config/local.thermite/settings.json"

    machine.wait_for_unit("thermite-fand.service")

    with subtest("the window can read the status file"):
        status = json.loads(machine.wait_until_succeeds("su alice -c 'cat /run/thermite/status.json'"))
        assert status["settings"] == settings, status
        pid = machine.succeed("systemctl show --property MainPID --value thermite-fand.service")
        assert status["pid"] == int(pid), (status, pid)

    with subtest("the daemon reads the settings in a home directory of mode 700"):
        assert machine.succeed("stat -c %a /home/alice").strip() == "700"
        machine.wait_until_succeeds(f"journalctl -u thermite-fand.service --grep 'cannot read {settings}: No such file'")
        machine.succeed(f"su alice -c 'mkdir -p ~/.config/local.thermite && echo nonsense > {settings}'")
        # "cannot parse", not "Permission denied": the daemon got to the text.
        machine.wait_until_succeeds(f"journalctl -u thermite-fand.service --grep 'cannot parse {settings}'")

    with subtest("the daemon gives way to fw-fanctrl"):
        machine.succeed("systemd-run --unit fw-fanctrl ${fwFanctrl}/bin/fw-fanctrl run")
        # The daemon checks for other fan controllers every 30 seconds.
        machine.wait_until_succeeds("""grep -F '"blockedBy":"fw-fanctrl"' /run/thermite/status.json""", timeout=60)
        machine.succeed("systemctl stop fw-fanctrl.service")
        machine.wait_until_succeeds("""grep -F '"blockedBy":null' /run/thermite/status.json""", timeout=60)

    with subtest("the daemon ran without a restart and stops cleanly"):
        assert machine.succeed("systemctl show --property NRestarts --value thermite-fand.service").strip() == "0"
        machine.succeed("systemctl stop thermite-fand.service")
        machine.fail("test -e /run/thermite")
        assert machine.succeed("systemctl show --property Result --value thermite-fand.service").strip() == "success"
  '';
}
