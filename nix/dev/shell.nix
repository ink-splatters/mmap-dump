{
  perSystem = {
    config,
    pkgs,
    ...
  }: let
    inherit (config) pre-commit craneLib;
  in {
    devShells.default =
      craneLib.devShell.override {
        mkShell = pkgs.mkShell.override {
          inherit (pkgs.llvmPackages_latest) stdenv;
        };
      } ({
          # TODO: don't seem to do anything, but crane docs say something that checks become available
          # at shell? or am I missing something?
          inherit (config) checks;

          packages = [pkgs.zizmor] ++ pre-commit.settings.enabledPackages;

          shellHook = ''
            ${pre-commit.installationScript}
          '';
        }
        // (builtins.removeAttrs config.commonArgsNative ["src" "stdenv"]));
  };
}
