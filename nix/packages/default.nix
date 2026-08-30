{
  perSystem = {config, ...}: let
    inherit
      (config)
      craneLib
      commonArgs
      commonArgsNative
      cargoArtifacts
      cargoArtifactsNative
      ;
  in {
    packages = {
      mmap-dump = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
        });

      mmap-dump-native = craneLib.buildPackage (commonArgsNative
        // {
          cargoArtifacts = cargoArtifactsNative;
          pnameSuffix = "-native";
        });
    };
  };
}
