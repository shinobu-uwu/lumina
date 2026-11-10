{
  description = "Lumina";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      crane,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib;
        craneLib = crane.mkLib pkgs;
        src = craneLib.cleanCargoSource ./.;
        commonArgs = {
          inherit src;
          strictDeps = true;
          nativeBuildInputs = [
            pkgs.pkg-config
          ]
          ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.clang ];
          buildInputs =
            lib.optionals pkgs.stdenv.hostPlatform.isLinux (
              with pkgs;
              [
                alsa-lib
                fontconfig
                freetype
                libxkbcommon
                wayland
                vulkan-loader
                xorg.libX11
                xorg.libXcursor
                xorg.libXi
                xorg.libXrandr
                xorg.libxcb
              ]
            )
            ++ lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];
          env = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
          };
        };
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        app = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoExtraArgs = "-p app";
            doCheck = false;
          }
        );
      in
      {
        packages.default = app;
        packages.app = app;
        apps.default = flake-utils.lib.mkApp {
          drv = app;
          exePath = "/bin/app";
        };

        checks = {
          taplo = craneLib.taploFmt {
            src = lib.sources.sourceFilesBySuffices src [ ".toml" ];
          };
          deny = craneLib.cargoDeny { inherit src; };
          nextest = craneLib.cargoNextest (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoNextestExtraArgs = "--no-tests=pass";
            }
          );

        };

        devShells.default = craneLib.devShell {
          inputsFrom = [ app ];
          packages = with pkgs; [
            taplo
            cargo-deny
            cargo-nextest
          ];
          shellHook = lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
            export LIBCLANG_PATH="${pkgs.libclang.lib}/lib"
            export LD_LIBRARY_PATH="${
              lib.makeLibraryPath [
                pkgs.libxkbcommon
                pkgs.wayland
                pkgs.vulkan-loader
              ]
            }:$LD_LIBRARY_PATH"
          '';
        };
      }
    );
}
