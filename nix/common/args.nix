{lib, ...}: {
  perSystem = {
    config,
    pkgs,
    ...
  }: let
    inherit (config) craneLib;
    inherit
      (pkgs.llvmPackages_latest)
      clang
      bintools
      libcxx
      stdenv
      ;
    inherit (pkgs) apple-sdk_15;

    mkFlags = flags: builtins.toString (map (x: "-C ${x}") flags);

    flags = [
      "linker=${clang}/bin/cc"
      "link-args=-fuse-ld=lld"
      "embed-bitcode=yes"
      "lto=thin"
    ];

    CFLAGS = "-O3 -pipe"; # TODO: or -O2/O3?
    LDFLAGS = "-fuse-ld=lld";
    mkFlagsNative = flags: lib.concatStringsSep "" [flags "-mcpu=native"];

    mkCommonArgs = args @ {flags, ...}:
      {
        src = craneLib.cleanCargoSource config.src;
        stdenv = _: stdenv;
        strictDeps = true;
        enableParallelBuilding = true;
        RUSTFLAGS = "-Zdylib-lto " + (mkFlags flags);

        buildInputs = [
          apple-sdk_15
          libcxx
        ];

        nativeBuildInputs = [
          clang
          bintools
        ];
        inherit CFLAGS LDFLAGS;
      }
      // (builtins.removeAttrs args ["flags"]);
  in {
    options = {
      commonArgs = lib.mkOption {
        type = lib.types.attrs;
        default = mkCommonArgs {inherit flags;};
      };

      commonArgsNative = lib.mkOption {
        type = lib.types.attrs;

        default = mkCommonArgs {
          flags = flags ++ ["target-cpu=native"];
          NIX_ENFORCE_NO_NATIVE = 0;

          CFLAGS = mkFlagsNative CFLAGS;
        };
      };
    };
  };
}
