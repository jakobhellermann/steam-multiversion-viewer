{
  description = "A tool for viewing contents of steam games at various game versions, including deep introspection and comparison.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane.url = "github:ipetkov/crane";

    flake-utils.url = "github:numtide/flake-utils";

  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        craneLib = crane.mkLib pkgs;

        cargoSrc = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            (craneLib.fileset.commonCargoSources ./.)
            ./THIRD-PARTY-NOTICES.html
            ./backend/static
          ];
        };

        frontendDist = pkgs.stdenvNoCC.mkDerivation {
          pname = "steam-multiversion-viewer-frontend";
          inherit (craneLib.crateNameFromCargoToml { cargoToml = ./Cargo.toml; }) version;
          src = pkgs.lib.fileset.toSource {
            root = ./frontend;
            fileset = pkgs.lib.fileset.unions [
              ./frontend/package.json
              ./frontend/pnpm-lock.yaml
              ./frontend/pnpm-workspace.yaml
              ./frontend/index.html
              ./frontend/vite.config.ts
              ./frontend/tsconfig.json
              ./frontend/public
              ./frontend/src
            ];
          };

          nativeBuildInputs = [
            pkgs.nodejs
            pkgs.pnpmConfigHook
            pkgs.pnpm_10
          ];

          pnpmDeps = pkgs.fetchPnpmDeps {
            pname = "steam-multiversion-viewer-frontend";
            inherit (craneLib.crateNameFromCargoToml { cargoToml = ./Cargo.toml; }) version;
            src = pkgs.lib.fileset.toSource {
              root = ./frontend;
              fileset = pkgs.lib.fileset.unions [
                ./frontend/package.json
                ./frontend/pnpm-lock.yaml
                ./frontend/pnpm-workspace.yaml
              ];
            };
            pnpm = pkgs.pnpm_10;
            fetcherVersion = 4;
            hash = "sha256-rtHKbY7ofSIIMl2Nbl4AqKTyRpknIHB1PB8wLSNSzOs=";
          };

          buildPhase = ''
            runHook preBuild
            pnpm build
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            cp -r dist $out
            runHook postInstall
          '';
        };

        src = pkgs.runCommand "src-with-frontend-dist" { } ''
          cp -r ${cargoSrc} $out
          chmod -R u+w $out
          mkdir -p $out/frontend
          cp -r ${frontendDist} $out/frontend/dist
        '';

        # Common arguments can be set here to avoid repeating them later
        # Note: changes here will rebuild all dependency crates
        commonArgs = {
          inherit src;
          pname = "steam-multiversion-viewer";
          strictDeps = true;

          nativeBuildInputs = [
            pkgs.pkg-config
          ];

          buildInputs = [
            pkgs.webkitgtk_4_1
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
            # Additional darwin specific inputs can be set here
            pkgs.libiconv
          ];
        };

        # Build *just* the cargo dependencies (of the entire workspace),
        # so we can reuse all of that work when running in CI
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        steam-multiversion-viewer = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.copyDesktopItems ];
            pname = "steam-multiversion-viewer";
            cargoExtraArgs = "-p steam-multiversion-viewer";

            desktopItems = [
              (pkgs.makeDesktopItem {
                name = "steam-multiversion-viewer";
                exec = "steam-multiversion-viewer --window";
                icon = "steam-multiversion-viewer";
                desktopName = "Steam Multiversion Viewer";
                categories = [ "Development" ];
              })
            ];

            postInstall = ''
              install -Dm644 ${./frontend/public/logo512.png} \
                $out/share/icons/hicolor/512x512/apps/steam-multiversion-viewer.png
            '';
          }
        );
      in
      {
        checks = {
          inherit steam-multiversion-viewer;
        };

        packages.default = steam-multiversion-viewer;

        apps.default = flake-utils.lib.mkApp {
          drv = steam-multiversion-viewer;
        };

        devShells.default = craneLib.devShell {
          # Inherit inputs from checks.
          checks = self.checks.${system};

          # Additional dev-shell environment variables can be set directly
          # MY_CUSTOM_DEVELOPMENT_VAR = "something else";
          packages = [
            # pkgs.ripgrep
          ];
        };
      }
    );
}
