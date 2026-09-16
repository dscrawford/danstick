{
  description = "Switch-style controller assignment, as stable virtual gamepads";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    (flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        # Test scaffolding only -- padmap itself is a Rust binary. These are
        # what `tools/` and the uinput fixtures need to stand a fake
        # controller up in front of the real thing.
        pythonEnv = pkgs.python3.withPackages (ps: with ps; [
          evdev # create the uinput pads the fixtures press
        ]);
        # Pinned explicitly: RetroArch's bundled joypad profiles are the source
        # we copy button mappings from when renaming a pad for a virtual one.
        autoconfig = pkgs.retroarch-joypad-autoconfig;
        autoconfigDir = "${autoconfig}/share/libretro/autoconfig";

        # The Rust port, under rust/. Taken from nixpkgs rather than through
        # fenix or rust-overlay: the workspace pins an edition and a
        # rust-version that nixpkgs' stable toolchain already satisfies, and a
        # second flake input to track is a cost with nothing bought by it.
        rustToolchain = [
          pkgs.cargo
          pkgs.rustc
          pkgs.clippy
          pkgs.rustfmt
          pkgs.rust-analyzer
          # Coverage. cargo-llvm-cov needs the llvm-tools that ship with
          # rustc, which nixpkgs puts in a separate output.
          pkgs.cargo-llvm-cov
          pkgs.cargo-nextest
        ];

        # `padmap` and `padmap-rs` as commands inside the dev shell, running
        # the working tree rather than a store copy.
        #
        # packages.padmap already exists, but it bakes in ${./src}: entering
        # the shell and editing a file would leave `padmap list` running the
        # source as it was when the flake was last evaluated. In a dev shell
        # that is the wrong answer every time, and a silent one -- the command
        # runs, it just is not your code.
        #
        # So these resolve from PADMAP_DEV_ROOT, which the shellHook pins to
        # the directory the shell was entered from. Pinned at entry rather
        # than read as $PWD per call, because `cd rust` must not change which
        # padmap you are running.
        devRoot = ''
          if [ -z "''${PADMAP_DEV_ROOT:-}" ]; then
            echo "padmap: PADMAP_DEV_ROOT is unset." >&2
            echo "padmap: this wrapper only works inside the dev shell." >&2
            exit 1
          fi
        '';

        # Builds on first use and after every edit. That is the point --
        # `padmap list` should never be stale -- but it means the first call
        # after touching a source file pauses to compile, and cargo writes its
        # progress to stderr so it is visible rather than a hang.
        #
        # PADMAP_BUILD_ID is deliberately *not* set here. packages.padmap sets
        # it to a store path, which is the right identity there because the
        # path changes with every edit. In the dev shell the binary is
        # rebuilt in place, so a value baked in at shell entry would make
        # `ensure-daemon` call a daemon current after you had edited under it
        # -- exactly the failure the build id exists to catch. Left unset, it
        # falls back to the binary's own mtime, which does change.
        devPadmap = pkgs.writeShellScriptBin "padmap" ''
          set -euo pipefail
          ${devRoot}
          exec cargo run --quiet --release \
            --manifest-path "$PADMAP_DEV_ROOT/rust/Cargo.toml" \
            --bin padmap-rs -- "$@"
        '';

        # Everything a crate that opens a device node needs to link -- and
        # SDL3, for the one question only SDL can answer: what its built-in
        # database says about a GUID. See padmap-input/src/sdlprobe.rs.
        rustBuildInputs = [ pkgs.udev pkgs.sdl3 ];
        rustNativeBuildInputs = [ pkgs.pkg-config ];

        # The whole workspace, built and tested in the sandbox.
        #
        # cargoLock.lockFile rather than a cargoHash: the lock file is in the
        # repo, so there is nothing to keep in step by hand and no hash to go
        # stale silently the next time a dependency is added.
        padmap-rs = pkgs.rustPlatform.buildRustPackage {
          pname = "padmap-rs";
          version = "0.1.0";
          src = ./rust;
          cargoLock.lockFile = ./rust/Cargo.lock;
          nativeBuildInputs = rustNativeBuildInputs;
          buildInputs = rustBuildInputs;
        };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = [
            pythonEnv
            pkgs.retroarch
            autoconfig
            pkgs.evsieve # reference implementation of evdev republishing
            pkgs.udev # udevadm, for inspecting ID_INPUT_JOYSTICK
            pkgs.sdl3 # linked by padmap-rs for `sdl-mapping`
            pkgs.evemu # replay a recorded device, for latency measurement
            pkgs.linuxPackages.perf # where the forwarding path actually goes
            devPadmap # `padmap ...`, built from the working tree
            # The launch wrapper as the real thing, so a `padmap launch` from
            # this shell takes the same path a packaged one does.
            self.packages.${system}.padmap-play
          ] ++ rustToolchain ++ rustNativeBuildInputs;

          # Without this the module falls back to globbing /nix/store, which
          # can pick an older autoconfig package at random.
          PADMAP_AUTOCONFIG_DIRS = autoconfigDir;

          shellHook = ''
            # Pinned once, at entry. The wrappers read this instead of $PWD so
            # that `cd rust` does not change which padmap `padmap list` runs.
            export PADMAP_DEV_ROOT="$PWD"
            # Without it a launch from this shell invokes a bare `retroarch`,
            # which looks like it worked.
            export PADMAP_PLAY="${self.packages.${system}.padmap-play}/bin/padmap-play"
            # cargo writes here; keeping it out of the source tree means a
            # `nix build` of the flake never sees a 2GB target/ in its source.
            export CARGO_HOME="''${CARGO_HOME:-$PWD/.cargo-home}"
            echo "padmap dev shell -- these build from $PADMAP_DEV_ROOT, not a store copy"
            echo "  padmap list          - what is plugged in"
            echo "  padmap setup         - assign player order"
            echo "  padmap run           - republish assigned pads"
            echo "  padmap map           - record which button is which"
            echo "  padmap launch        - republish, then start RetroArch"
            echo "  padmap hide          - udev rules hiding the physical pads (root)"
            echo "  padmap play          - resolve mappings for a game (padmap-play)"
            echo "  padmap exec -- CMD   - run CMD with padmap's mappings (Cemu, ...)"
            echo "  padmap --help        - the rest"
            echo "  (cd rust && cargo test)  - the tests"
            echo "  (cd rust && cargo clippy --all-targets -- -D warnings)"
            echo
            if [ ! -w /dev/uinput ]; then
              echo "WARNING: /dev/uinput is not writable by $(id -un)."
              echo "  The virtual-pad daemon needs it. On this machine it is granted"
              echo "  by a logind seat ACL, so check you are on an active session."
            fi
          '';
        };

        packages.padmap = pkgs.writeShellApplication {
          name = "padmap";
          runtimeInputs = [ pkgs.udev pkgs.retroarch ];
          text = ''
            # Identity of the code being run. The store path changes with
            # every source edit, which is what lets a client notice that a
            # long-running daemon is still on the previous version --
            # something nothing else about it reveals.
            export PADMAP_BUILD_ID="${padmap-rs}"
            export PADMAP_AUTOCONFIG_DIRS="${autoconfigDir}"
            # Absolute, so generated launch commands work from a front-end
            # that has neither padmap nor RetroArch on its PATH.
            export PADMAP_PLAY="${self.packages.${system}.padmap-play}/bin/padmap-play"
            exec ${padmap-rs}/bin/padmap-rs "$@"
          '';
        };

        packages.padmap-play = pkgs.writeShellApplication {
          name = "padmap-play";
          runtimeInputs = [ pkgs.retroarch ];
          text = ''
            state="''${XDG_RUNTIME_DIR:-/tmp}/padmap"
            config="$state/launch.cfg"
            argsfile="$state/launch.args"

            # This wrapper is the only place that knows what is about to be
            # played: it is handed `-L <core.so>` and the ROM path, and the
            # daemon wrote its autoconfig profiles long before, when nothing
            # could know either. So the mapping a controller uses for *this*
            # console or *this* game is resolved here, rewriting the same
            # autoconfig directory the launch override already points at.
            #
            # padmap itself rather than a second copy in shell. Deciding a
            # console from a core name and a stable key from a ROM path are
            # table lookups that already exist on the padmap side, and a
            # shell copy would be a table with nothing to notice when it fell
            # behind -- the failure this project has hit with the launcher
            # path, the theme link and the daemon itself.
            #
            # Never fatal: the default profiles are already on disk, so the
            # worst case is the mapping padmap wrote before scopes existed.
            ${padmap-rs}/bin/padmap-rs play -- "$@" \
              || echo "padmap: mapping resolution failed; using defaults" >&2

            # Flags that cannot be expressed as config settings, one token
            # per line. Emptying an unassigned core port is the only thing
            # in here so far: RetroArch ignores input_libretro_device_pN
            # from a config file and honours only --nodevice PORT.
            args=()
            if [ -f "$argsfile" ]; then
              while IFS= read -r line; do
                [ -n "$line" ] && args+=("$line")
              done < "$argsfile"
            fi

            # Tell the daemon a game is running, so that plugging in a new
            # controller mid-game does not open the setup screen -- which
            # would stop republishing and grab every pad, leaving the player
            # holding a controller that has quietly stopped working.
            #
            # This is why nothing here is exec'd any more: something has to
            # outlive RetroArch to remove the marker. It holds our pid, so a
            # padmap-play that is killed outright cannot disable the feature
            # until the next reboot.
            mkdir -p "$state"
            marker="$state/playing"
            printf '%s\n' "$$" > "$marker"
            trap 'rm -f "$marker"' EXIT INT TERM

            status=0

            # Always leave a log behind.
            #
            # RetroArch was being run with no logging flags, so every launch
            # was invisible: when a controller did not work in game there was
            # nothing to read, and the only file with the right name was a
            # stale one from a manual run days earlier -- which is worse than
            # none, because it looks like evidence. The [Autoconf] lines here
            # are the only place that says which pad RetroArch matched, in
            # which port, and against which profile.
            #
            # Truncated per launch by --log-file, so this cannot grow without
            # bound; the previous run is available until the next one starts.
            logfile="$state/retroarch.log"

            # Only pass the override if it exists: before the first
            # assignment there is no file, and RetroArch treats a missing
            # --appendconfig target as a fatal error.
            if [ -f "$config" ]; then
              retroarch --verbose --log-file "$logfile" \
                   --appendconfig "$config" \
                   ''${args[@]+"''${args[@]}"} "$@" || status=$?
            else
              echo "padmap: no launch config at $config;" \
                   "controller order will be RetroArch's default" >&2
              retroarch --verbose --log-file "$logfile" "$@" || status=$?
            fi

            exit "$status"
          '';
        };

        # One command for "bring the machine up": make the daemon current and
        # refresh the udev rules. It launches nothing afterwards -- padmap is a
        # virtual gamepad, and what reads the pads it publishes is the user's
        # business. Run it from a session manager or a .desktop file before
        # whatever does.
        packages.padmap-start = pkgs.writeShellApplication {
          name = "padmap-start";
          text = ''
            if [ "''${PADMAP_SKIP_DAEMON_CHECK:-0}" != "1" ]; then
              ${self.packages.${system}.padmap}/bin/padmap ensure-daemon \
                || echo "padmap: continuing without a current daemon" >&2
            fi

            # Refresh the udev rules that hide the physical pads from anything
            # else enumerating them.
            #
            # They go stale in two ways, and both end in the same baffling
            # symptom. They live in /run/udev/rules.d, which is tmpfs, so a
            # reboot removes them entirely; and they only ever name the
            # controllers that were plugged in when they were written, so any
            # pad bought since is not covered. Either way a front-end sees the
            # *physical* pad as well as padmap's clone, and a wizard that asks
            # the user to press B has that B delivered to the UI as a cancel --
            # the configuration screen closes on the button it just requested,
            # with nothing anywhere saying why.
            #
            # -n so it can never sit at a password prompt on a machine that is
            # plugged into a television. If it cannot elevate, say what to do
            # and carry on.
            if [ "''${PADMAP_SKIP_HIDE:-0}" != "1" ]; then
              if ! sudo -n ${self.packages.${system}.padmap}/bin/padmap hide \
                     >/dev/null 2>&1; then
                echo "padmap: could not refresh the udev hide rules (needs root)." >&2
                echo "padmap: run 'sudo padmap hide' once, or set" >&2
                echo "padmap:   programs.padmap.autoHide = true;" >&2
                echo "padmap: so they are kept current automatically." >&2
              fi
            fi

            # Anything started after this need not repeat the check.
            export PADMAP_SKIP_DAEMON_CHECK=1
          '';
        };

        packages.paddump = pkgs.writeShellApplication {
          name = "paddump";
          runtimeInputs = [ pythonEnv pkgs.udev ];
          text = ''exec python3 ${./tools/paddump.py} "$@"'';
        };

        # The controller artwork, for whatever draws a setup screen. Named
        # by `icons.ICON_NAMES`, stored in a profile, and sent over the socket,
        # so a client needs the files even though padmap never renders them.
        packages.icons = pkgs.runCommand "padmap-icons" { } ''
          mkdir -p "$out/share/padmap/icons"
          cp ${./assets/icons}/*.svg "$out/share/padmap/icons/"
        '';

        packages.default = self.packages.${system}.padmap;

        packages.padmap-rs = padmap-rs;

        # padmap itself. Kept under this name as well as `packages.padmap`
        # so `nix run .#padmap-rs` still works for anyone who scripted it.
        apps.padmap-rs = {
          type = "app";
          program = "${padmap-rs}/bin/padmap-rs";
        };

        # buildRustPackage runs `cargo test` in its checkPhase, so building
        # this is running the suite. Separate from checks.rust-lint so a
        # clippy opinion cannot be mistaken for a failing test.
        checks.rust = padmap-rs;

        checks.rust-lint = pkgs.rustPlatform.buildRustPackage {
          pname = "padmap-rs-lint";
          version = "0.1.0";
          src = ./rust;
          cargoLock.lockFile = ./rust/Cargo.lock;
          nativeBuildInputs = rustNativeBuildInputs ++ [ pkgs.clippy pkgs.rustfmt ];
          buildInputs = rustBuildInputs;
          buildPhase = ''
            cargo fmt --all --check
            cargo clippy --all-targets --all-features -- -D warnings
          '';
          # buildRustPackage's default check and install phases both want a
          # binary this derivation does not produce.
          doCheck = false;
          installPhase = "touch $out";
        };
      })) // {

      # System-level bits: the udev rules that hide physical pads cannot be
      # installed imperatively on NixOS, since /etc/udev/rules.d is a symlink
      # into the store.
      nixosModules.padmap = { config, lib, pkgs, ... }:
        let
          cfg = config.programs.padmap;

          # "0079:1879" -> a rule clearing ID_INPUT_JOYSTICK for that device.
          # Matching on an explicit vid/pid rather than anything broader is
          # deliberate: a rule that caught every joystick would leave the
          # machine with no usable controllers whenever padmap is not running.
          hideRule = spec:
            let
              parts = lib.splitString ":" spec;
              vendor = lib.elemAt parts 0;
              product = lib.elemAt parts 1;
            in
            assert lib.assertMsg (lib.length parts == 2)
              "programs.padmap.hideDevices: expected \"vvvv:pppp\", got ${spec}";
            ''SUBSYSTEM=="input", ATTRS{idVendor}=="${vendor}", ATTRS{idProduct}=="${product}", ENV{ID_INPUT_JOYSTICK}=""'';
        in
        {
          options.programs.padmap = {
            enable = lib.mkEnableOption "padmap controller assignment";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.padmap;
              defaultText = lib.literalExpression "padmap.packages.\${system}.padmap";
              description = "The padmap package to install.";
            };

            hideDevices = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [ ];
              example = [ "0079:1879" "0079:1830" ];
              description = ''
                USB `vendor:product` pairs, lowercase hex, to hide from
                RetroArch's joypad enumeration by clearing ID_INPUT_JOYSTICK.

                Get the list for the currently connected pads by running
                `padmap hide`.

                While a device is hidden it is invisible to RetroArch unless
                padmap is running and republishing it, so listing a controller
                here is a commitment to launching games through padmap.
              '';
            };

            autoHide = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = ''
                Keep the hide rules current by regenerating them whenever a
                joypad appears, instead of maintaining `hideDevices` by hand.

                `hideDevices` is a list written once, and it goes stale the
                day a new controller is plugged in: that pad keeps its
                ID_INPUT_JOYSTICK, so the front-end enumerates the *physical*
                device alongside padmap's clone. The visible symptom is
                nowhere near udev -- the mapping wizard asks the user to press
                B, the front-end receives the same B as a cancel, and the
                configuration screen closes on the button it just asked for.

                With this on, a oneshot service runs `padmap hide` at boot and
                on every joypad hotplug. That command rewrites the rules from
                what is actually connected and reloads udev only when the file
                changed, so the udev trigger cannot drive itself in a loop.

                Same commitment as `hideDevices`: while the rules are active
                and padmap is not running, the covered controllers are
                invisible to RetroArch.
              '';
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];

            systemd.services.padmap-hide = lib.mkIf cfg.autoHide {
              description = "Refresh padmap's udev rules for the connected pads";
              # At boot as well as on hotplug: the rules live in
              # /run/udev/rules.d, which is tmpfs, so every reboot starts with
              # none of them and every controller visible again.
              wantedBy = [ "multi-user.target" ];
              serviceConfig = {
                Type = "oneshot";
                ExecStart = "${cfg.package}/bin/padmap hide";
              };
            };

            services.udev.extraRules =
              lib.optionalString (cfg.hideDevices != [ ]) ''
                # Generated by the padmap NixOS module.
                ${lib.concatMapStringsSep "\n" hideRule cfg.hideDevices}
              ''
              + lib.optionalString cfg.autoHide ''
                # Regenerate padmap's hide rules whenever a joypad appears.
                # `padmap hide` is a no-op when the rules already match, so
                # the reload it would otherwise trigger cannot re-enter this.
                SUBSYSTEM=="input", ACTION=="add", ENV{ID_INPUT_JOYSTICK}=="1", TAG+="systemd", ENV{SYSTEMD_WANTS}+="padmap-hide.service"
              '';
          };
        };

      nixosModules.default = self.nixosModules.padmap;
    };
}
