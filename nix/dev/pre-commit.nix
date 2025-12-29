{
  perSystem = {
    config,
    pkgs,
    ...
  }: {
    pre-commit = {
      check.enable = true;

      settings.hooks = {
        # TOML/Cargo files
        check-toml.enable = true;

        # Markdown
        markdownlint = {
          enable = true;
          settings.configuration = {
            MD013 = false; # Disable line length
            MD033 = false; # Allow inline HTML
            MD040 = false; # Don't require language for code blocks
          };
        };

        # Spell checking
        typos.enable = true;

        # Nix hooks
        deadnix.enable = true;
        nil.enable = true;
        alejandra.enable = true;
        statix.enable = true;

        # dockerfile check and formatting
        dockerfmt-check = {
          enable = true;
          name = "Check Dockerfile";
          description = "Run 'dockerfmt -c'";
          entry = "dockerfmt -c";
          types = ["dockerfile"];
          require_serial = true;
          pass_filenames = true;
          extraPackages = [pkgs.dockerfmt];
        };

        dockerfmt = {
          enable = true;
          name = "Format Dockerfile";
          description = "Run 'dockerfmt -w'";
          entry = "dockerfmt -w";
          types = ["dockerfile"];
          require_serial = true;
          pass_filenames = true;
          extraPackages = [pkgs.dockerfmt];
        };
      };
    };

    apps.install-hooks = {
      type = "app";
      program = toString (pkgs.writeShellScript "install-hooks" ''
        ${config.pre-commit.installationScript}
        echo "Pre-commit hooks installed!"
      '');
      meta.description = "install pre-commit hooks";
    };
  };
}
