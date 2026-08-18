{
  description = "Logitech TrueForce Linux driver (RS50 / G PRO / G923) - kernel module + userspace tools";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
      let
        version = (builtins.fromTOML (builtins.readFile (self + "/userspace/logi-wheel/Cargo.toml"))).workspace.package.version; 

        logitechTrueforceModuleFor = pkgs: { kernel, debug ? false }:
        let
          src = self; 
        in
        pkgs.stdenv.mkDerivation {
          pname = "logitech-trueforce-driver";
          inherit version src;
          nativeBuildInputs = kernel.moduleBuildDependencies;
          makeFlags = [
            "KDIR=${kernel.dev}/lib/modules/${kernel.modDirVersion}/build"
          ] ++ pkgs.lib.optionals debug [ "DEBUG=1" ];
          buildPhase = ''
            runHook preBuild
            (cd mainline && make $makeFlags all)
            runHook postBuild
          '';
          installPhase = ''
            moddir=$out/lib/modules/${kernel.modDirVersion}/extra
            mkdir -p "$moddir"
            cp mainline/hid-logitech-dd.ko "$moddir/"
          '';
          meta = with pkgs.lib; {
            description = "Kernel module for Logitech TrueForce wheels (RS50, G PRO, G923)";
            homepage = "https://github.com/mescon/logitech-trueforce-linux-driver";
            license = licenses.gpl2Only;
            platforms = platforms.linux;
          };
        };
    in
    flake-utils.lib.eachSystem [ "x86_64-linux" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };

        src = self; 

        logitechTrueforceModule = logitechTrueforceModuleFor pkgs;
        
        logiWheel = pkgs.rustPlatform.buildRustPackage {
          pname = "logi-wheel";
          inherit version src;
          buildAndTestSubdir = "userspace/logi-wheel";
          cargoRoot = "userspace/logi-wheel";
          doCheck = false;
          nativeBuildInputs = [ pkgs.pkg-config pkgs.gnumake pkgs.makeWrapper ];
          cargoLock = {
                lockFile = self + "/userspace/logi-wheel/Cargo.lock";
                };

          buildInputs = [ pkgs.fontconfig ]
            ++ pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.libxkbcommon pkgs.wayland ]
            ++ [ pkgs.libx11 pkgs.libxcursor pkgs.libxi pkgs.libxrandr ];

          postInstall = ''
                install -Dm644 desktop/logi-wheel-gui.desktop \
                        $out/share/applications/logi-wheel-gui.desktop
                install -Dm644 userspace/logi-wheel/crates/logi-wheel-gui/ui/assets/logo-mark.png \
                        $out/share/pixmaps/logi-wheel-gui.png

                # The pieces a game loads rather than the user running, which
                # every other packaging channel installs here (see
                # packaging/debian/rules). Without them the Setup page's
                # "Install TrueForce" and "Install relay" buttons find
                # nothing to copy, and the truck sims and every shared-memory
                # sim stay silent, so leaving them out makes the install
                # quietly incomplete rather than obviously broken.
                #
                # The relay is a prebuilt Windows executable: it runs inside
                # a game's Proton prefix, and no Linux toolchain here can
                # produce one. The proxy DLL is prebuilt for the same reason.
                install -Dm644 tools/tf-range-proxy.dll \
                        $out/share/logitech-trueforce/tf-range-proxy.dll
                install -Dm644 tools/logi-tf-relay.exe \
                        $out/share/logitech-trueforce/logi-tf-relay.exe
                # The recorded TrueForce init burst logi-launch replays when
                # LOGI_TF_REARM is set. Small, and the alternative is a
                # feature that silently cannot work on this channel only.
                install -Dm644 tools/tf-init.bin \
                        $out/share/logitech-trueforce/tf-init.bin
                # The dinput8 escape proxy logi-launch stages into an SDK
                # game's own directory: it answers the SDK's range getters
                # and relays the game's RPM telemetry for the kernel
                # texture merge. Prebuilt here too, and deliberately NOT
                # cross-built via pkgsCross.mingwW64: that would hand
                # every build of this package a mingw toolchain to
                # produce a binary that is not the hardware-validated
                # artifact the other channels ship. CI keeps the
                # committed DLL in step with its source
                # (tools/check-committed-dlls.sh).
                install -Dm644 tools/dinput8-escape.dll \
                        $out/share/logitech-trueforce/dinput8-escape.dll
                # Built from this workspace as a cdylib, so it is already in
                # $out/lib; the games load it by path from share/. Symlinked
                # rather than copied to avoid duplicating.
                ln -s $out/lib/liblogi_tf_scs.so \
                        $out/share/logitech-trueforce/liblogi_tf_scs.so
                # The TrueForce shim installer, under the name the app looks
                # for first (logi_wheel_core::helpers::INSTALLER_BINS).
                install -Dm755 tools/install-tf-shim.sh $out/bin/logi-shim
                # The RPM feed for the kernel texture merge; logi-launch
                # starts and stops it around a game session. Built the
                # same way packaging/debian/rules builds it.
                cc -O2 -Wall -o $out/bin/logi-rpm-bridge tools/logi-rpm-bridge.c
                # The Steam launch-options wrapper. The whole feature is
                # that a user types `logi-launch %command%` and nothing
                # else, so it must be on PATH beside the helpers it
                # starts. It looks for the dinput8 proxy under /usr/share,
                # which does not exist on NixOS; point it at this
                # package's share directory.
                install -Dm755 tools/logi-launch.sh $out/bin/logi-launch
                substituteInPlace $out/bin/logi-launch \
                  --replace-fail "/usr/share/logitech-trueforce/dinput8-escape.dll" \
                                 "$out/share/logitech-trueforce/dinput8-escape.dll"
                # Rebinds a wheel that another driver claimed, which the
                # settings apps' diagnostics offer as the fix.
                install -Dm755 tools/rebind-wheel.sh $out/bin/logi-rebind-wheel
                '';

          postFixup = ''
            # install-tf-shim.sh is a bash script that shells out; give it
            # its tools rather than relying on the user's PATH.
            patchShebangs $out/bin/logi-shim
            wrapProgram $out/bin/logi-shim \
              --prefix PATH : ${pkgs.lib.makeBinPath [
                pkgs.coreutils
                pkgs.findutils
                pkgs.gnused
                pkgs.gnugrep
                pkgs.gawk
              ]}

            # logi-launch shells out too, and additionally starts the
            # sibling binaries from this package (logi-wheel, logi-tf-sim,
            # logi-ffb, logi-rpm-bridge), so its own bin directory leads
            # the wrapped PATH. python3 sends the wheel's TrueForce
            # teardown pair after an SDK session; setsid detaches the
            # daemon; pgrep and cmp are the script's own checks.
            patchShebangs $out/bin/logi-launch $out/bin/logi-rebind-wheel
            wrapProgram $out/bin/logi-launch \
              --prefix PATH : $out/bin:${pkgs.lib.makeBinPath [
                pkgs.coreutils
                pkgs.diffutils
                pkgs.gnused
                pkgs.gnugrep
                pkgs.procps
                pkgs.python3
                pkgs.util-linux
              ]}

            wrapProgram $out/bin/logi-wheel-gui \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [
                pkgs.wayland
                pkgs.libxkbcommon
                pkgs.libx11
                pkgs.libxcursor
                pkgs.libxi
                pkgs.libxrandr
                pkgs.libGL
                pkgs.vulkan-loader
                pkgs.libglvnd
              ]}
          '';

          meta = with pkgs.lib; {
            description = "Userspace CLI/TUI/GUI tools for the Logitech TrueForce driver";
            homepage = "https://github.com/mescon/logitech-trueforce-linux-driver";
            # Mixed by design: the Slint GUI is GPL-3.0-or-later, everything
            # else in the workspace is GPL-2.0-only.
            license = [ licenses.gpl2Only licenses.gpl3Plus ];
            platforms = platforms.linux;
            mainProgram = "logi-wheel";
          };
        };

        logiG923Modeswitch = pkgs.stdenv.mkDerivation {
          pname = "logi-wheel-modeswitch";
          inherit version src;

          nativeBuildInputs = [ pkgs.makeWrapper ];
          dontBuild = true;

          installPhase = ''
            mkdir -p $out/bin
            cp tools/xbox-modeswitch.sh $out/bin/logi-wheel-modeswitch
            chmod +x $out/bin/logi-wheel-modeswitch
            wrapProgram $out/bin/logi-wheel-modeswitch \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.usb-modeswitch ]}
          '';

          meta = with pkgs.lib; {
            description = "Switches a Logitech G923 (Xbox edition) out of console mode";
            homepage = "https://github.com/mescon/logitech-trueforce-linux-driver";
            license = licenses.gpl2Only;
            platforms = platforms.linux;
            mainProgram = "logi-wheel-modeswitch";
          };
        };

        udevRules = pkgs.runCommand "logitech-trueforce-udev-rules" { } ''
          mkdir -p $out/lib/udev/rules.d
          cp ${src}/udev/70-logitech-trueforce.rules $out/lib/udev/rules.d/
          cp ${src}/udev/71-logi-ffb-uhid.rules $out/lib/udev/rules.d/
          cp ${src}/udev/72-logitech-g923-rebind.rules $out/lib/udev/rules.d/
          cp ${src}/udev/73-logitech-xbox-modeswitch.rules $out/lib/udev/rules.d/

          substituteInPlace $out/lib/udev/rules.d/70-logitech-trueforce.rules --replace-quiet "/bin/sh" "${pkgs.runtimeShell}"
          substituteInPlace $out/lib/udev/rules.d/72-logitech-g923-rebind.rules --replace-quiet "/bin/sh" "${pkgs.runtimeShell}"
          substituteInPlace $out/lib/udev/rules.d/73-logitech-xbox-modeswitch.rules --replace-quiet "/usr/bin/logi-wheel-modeswitch" "${logiG923Modeswitch}/bin/logi-wheel-modeswitch"
          substituteInPlace $out/lib/udev/rules.d/73-logitech-xbox-modeswitch.rules --replace-quiet "/bin/sh" "${pkgs.runtimeShell}"
        '';

      in
      {
        packages = {
          default = logiWheel;
          logi-wheel = logiWheel;
          kernel-module = logitechTrueforceModule { kernel = pkgs.linuxPackages.kernel; };
          logi-wheel-modeswitch = logiG923Modeswitch;
          udev-rules = udevRules;
        };

        checks = {
                tests = logiWheel.overrideAttrs (old: {
                doCheck = true;
                checkFlags = [ "--skip" "setup_t_arms_consent_and_only_y_plays" ];
                });
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [ pkgs.pkg-config pkgs.cargo pkgs.rustc pkgs.rust-analyzer ];
          buildInputs = [
            pkgs.fontconfig
            pkgs.libxkbcommon
            pkgs.wayland
            pkgs.libx11
            pkgs.libxcursor
            pkgs.libxi
            pkgs.libxrandr
          ];
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [ pkgs.libGL pkgs.vulkan-loader ];
        };

        formatter = pkgs.nixpkgs-fmt;
      }
    ) // {
      nixosModules.default = { config, lib, pkgs, ... }:
        let 
          cfg = config.hardware.logitech-trueforce;
          logitechTrueforceModule = logitechTrueforceModuleFor pkgs;
        in {
          options.hardware.logitech-trueforce.enable = lib.mkEnableOption "Logitech TrueForce wheel driver";

          config = lib.mkIf cfg.enable {
            boot.extraModulePackages = [
                (logitechTrueforceModule {kernel = config.boot.kernelPackages.kernel; })
                ];
            boot.kernelModules = [ "hid-logitech-dd" ];
            # The same two lines packaging/modprobe.d/hid-logitech-dd.conf
            # carries on every other channel, which NixOS cannot take as a
            # file: the load-order hint that lets this driver claim a wheel
            # before the in-tree ones do, and the narrow blacklist that
            # stops berarma's new-lg4ff fork racing it for the G923 ids.
            # Without them a NixOS G923 owner depends on the udev rebind
            # rule alone, which is meant to be the fallback.
            boot.extraModprobeConfig = ''
              softdep hid-logitech-dd post: hid-logitech hid-logitech-hidpp
              blacklist hid-logitech-new
            '';
            services.udev.packages = [ self.packages.${pkgs.system}.udev-rules ];
            environment.systemPackages = [
              self.packages.${pkgs.system}.logi-wheel 
              self.packages.${pkgs.system}.logi-wheel-modeswitch 
            ];
          };
        };
    };
}
