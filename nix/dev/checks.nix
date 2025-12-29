{
  perSystem = {config, ...}: let
    inherit (config) craneLib commonArgs src cargoArtifacts;
  in {
    checks = {
      inherit (config.packages) mmap-dump;

      mmap-dump-clippy = craneLib.cargoClippy (
        commonArgs
        // {
          inherit cargoArtifacts;
          cargoClippyExtraArgs = "--all-targets -- --deny warnings";
        }
      );

      mmap-dump-doc = craneLib.cargoDoc (
        commonArgs
        // {
          inherit cargoArtifacts;
        }
      );

      mmap-dump-fmt = craneLib.cargoFmt {
        inherit src;
      };

      mmap-dump-nextest = craneLib.cargoNextest (
        commonArgs
        // {
          inherit cargoArtifacts;
          partitions = 1;
          partitionType = "count";
        }
      );
    };
  };
}
