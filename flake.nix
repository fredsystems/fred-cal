{
  description = "Dev shell and Linting";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    precommit = {
      url = "github:FredSystems/pre-commit-checks";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      precommit,
      ...
    }:
    let
      systems = precommit.lib.supportedSystems;
      inherit (nixpkgs) lib;
    in
    {
      ##########################################################################
      ## PACKAGES
      ##########################################################################
      packages = lib.genAttrs systems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        rec {
          fred-cal = pkgs.rustPlatform.buildRustPackage {
            pname = "fred-cal";
            version = "0.1.0";

            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [
              pkgs.pkg-config
            ];

            meta = with pkgs.lib; {
              description = "A calendar (CalDAV) syncing tool";
              homepage = "https://github.com/fredsystems/fred-cal";
              license = licenses.mit;
              platforms = platforms.linux;
              maintainers = [ maintainers.fredclausen ];
            };
          };

          default = fred-cal;
        }
      );

      ##########################################################################
      ## APPS (nix run .)
      ##########################################################################
      apps = lib.genAttrs systems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/fred-cal";
        };
      });

      ##########################################################################
      ## CHECKS
      ##########################################################################
      checks = lib.genAttrs systems (system: {
        pre-commit = precommit.lib.mkCheck {
          inherit system;
          src = ./.;

          check_rust = true;
          check_docker = false;
          check_python = false;

          enableXtask = true;

          python = {
            enableBlack = true;
            enableFlake8 = true;
          };
        };
      });

      ##########################################################################
      ## DEV SHELLS
      ##########################################################################
      devShells = lib.genAttrs systems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          chk = self.checks.${system}.pre-commit;
        in
        {
          default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                markdownlint-cli2
                cargo-deny
                cargo-machete
                typos
              ]
              ++ (chk.passthru.devPackages or [ ])
              ++ chk.enabledPackages;

            shellHook = ''
              # Run git-hooks.nix / pre-commit setup
              ${chk.shellHook}

              # Your own extras
              alias pre-commit="pre-commit run --all-files"
              alias xtask="cargo run -p xtask --"
            '';
          };
        }
      );

    };
}
