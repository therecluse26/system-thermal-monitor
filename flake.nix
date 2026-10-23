{
  description = "Thermite's fan daemon, thermite-fand, and a NixOS module that starts it at boot";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      # x86_64 only, like the release packages.
      forEachSystem =
        f: nixpkgs.lib.genAttrs [ "x86_64-linux" ] (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forEachSystem (pkgs: {
        thermite-fand = pkgs.callPackage ./nix/package.nix { };
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.thermite-fand;
      });

      nixosModules.default = ./nix/module.nix;

      checks = forEachSystem (pkgs: {
        inherit (self.packages.${pkgs.stdenv.hostPlatform.system}) thermite-fand;
        module = pkgs.callPackage ./nix/test.nix { };
      });

      formatter = forEachSystem (pkgs: pkgs.nixfmt);
    };
}
