# The fan daemon, thermite-fand, built from this repository. It builds only the
# thermite-core crate, not the window.
{ lib, rustPlatform }:

let
  workspace = ../src-tauri;
in
rustPlatform.buildRustPackage {
  pname = "thermite-fand";
  version = (lib.importTOML (workspace + "/core/Cargo.toml")).package.version;

  # Cargo reads the manifest of each workspace member, so the app's Cargo.toml and src/
  # come too. The rest of the app stays out, so a change there does not rebuild the daemon.
  src = lib.fileset.toSource {
    root = workspace;
    fileset = lib.fileset.unions [
      (workspace + "/Cargo.toml")
      (workspace + "/Cargo.lock")
      (workspace + "/src")
      (workspace + "/core")
    ];
  };
  cargoLock.lockFile = workspace + "/Cargo.lock";

  cargoBuildFlags = [
    "--package"
    "thermite-core"
    "--bin"
    "thermite-fand"
  ];
  cargoTestFlags = [
    "--package"
    "thermite-core"
  ];
  # The build sandbox has its own /etc/passwd, which gives root the home /build, not /root.
  checkFlags = [ "--skip=daemon::tests::root_has_a_home" ];

  meta = {
    description = "Thermite's fan daemon: runs one user's fan settings from boot";
    homepage = "https://github.com/therecluse26/thermite";
    platforms = lib.platforms.linux;
    mainProgram = "thermite-fand";
  };
}
