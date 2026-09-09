{
  description = "Switch-style controller assignment and launcher for RetroArch";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    (flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        pythonEnv = pkgs.python3.withPackages (ps: with ps; [
          evdev # raw /dev/input access + uinput device creation
          pysdl2 # cross-check SDL enumeration order against udev's
          pyside6 # QML front-end
          # In the same env as the libraries above, so it can see their stubs
          # -- PySide6 ships .pyi files that mypy only finds from inside.
          mypy
        ]);
        # Pinned explicitly: RetroArch's bundled joypad profiles are the source
        # we copy button mappings from when renaming a pad for a virtual one.
        autoconfig = pkgs.retroarch-joypad-autoconfig;
        autoconfigDir = "${autoconfig}/share/libretro/autoconfig";

        # nixpkgs' pyside6 links against Qt from the store rather than
        # bundling it, and nothing sets these for a bare python3 invocation.
        # Without them the QML engine reports even "QtQuick" as not installed.
        qtPluginPath = "${pkgs.qt6.qtbase}/${pkgs.qt6.qtbase.qtPluginPrefix}";
        qtQmlPath = "${pkgs.qt6.qtdeclarative}/${pkgs.qt6.qtbase.qtQmlPrefix}";
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = [
            pythonEnv
            pkgs.retroarch
            autoconfig
            pkgs.evsieve # reference implementation of evdev republishing
            pkgs.udev # udevadm, for inspecting ID_INPUT_JOYSTICK
          ];

          # Without this the module falls back to globbing /nix/store, which
          # can pick an older autoconfig package at random.
          PADMAP_AUTOCONFIG_DIRS = autoconfigDir;

          QT_PLUGIN_PATH = qtPluginPath;
          QML2_IMPORT_PATH = qtQmlPath;

          shellHook = ''
            export PYTHONPATH="$PWD/src''${PYTHONPATH:+:$PYTHONPATH}"
            # The same two the `padmap` wrapper sets. Without them an
            # export-pegasus run from this shell silently writes a bare
            # `retroarch` launch line and resolves no MAME set names -- it
            # looks like it worked and produces a library that cannot launch
            # anything and shows 8302 raw set names.
            export PADMAP_MAME_TITLES="${self.packages.${system}.mame-titles}/share/padmap/mame-titles.json"
            export PADMAP_PLAY="${self.packages.${system}.padmap-play}/bin/padmap-play"
            echo "padmap dev shell"
            echo "  python3 -m padmap.cli list    - what is plugged in"
            echo "  python3 -m padmap.cli setup   - assign player order"
            echo "  python3 -m padmap.cli run     - republish assigned pads"
            echo "  nix run .#padmap-start        - start everything (daemon + Pegasus)"
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
          runtimeInputs = [ pythonEnv pkgs.udev pkgs.retroarch ];
          # wrapQtAppsHook does not apply to a shell wrapper around a Python
          # entry point, so the Qt plugin path is set explicitly. Without it
          # the QML engine starts but finds no platform plugin.
          text = ''
            export PYTHONPATH="${./src}''${PYTHONPATH:+:$PYTHONPATH}"
            # Identity of the code being run. The store path changes with
            # every source edit, which is what lets a client notice that a
            # long-running daemon is still on the previous version --
            # something nothing else about it reveals.
            export PADMAP_BUILD_ID="${./src}"
            export PADMAP_AUTOCONFIG_DIRS="${autoconfigDir}"
            export PADMAP_MAME_TITLES="${self.packages.${system}.mame-titles}/share/padmap/mame-titles.json"
            # Absolute, so generated launch commands work from a front-end
            # that has neither padmap nor RetroArch on its PATH.
            export PADMAP_PLAY="${self.packages.${system}.padmap-play}/bin/padmap-play"
            export QT_PLUGIN_PATH="${qtPluginPath}''${QT_PLUGIN_PATH:+:$QT_PLUGIN_PATH}"
            export QML2_IMPORT_PATH="${qtQmlPath}''${QML2_IMPORT_PATH:+:$QML2_IMPORT_PATH}"
            exec python3 -m padmap.cli "$@"
          '';
        };

        # Pegasus with the padmap API exposed to QML as `Api.padmap`.
        #
        # The patch is deliberately tiny -- five lines across three existing
        # files plus one new directory -- because all the device handling
        # stays in the padmap daemon and this is only a socket client. That
        # keeps rebasing onto new upstream revisions cheap.
        packages.pegasus =
          let
            patched = pkgs.pegasus-frontend.overrideAttrs (old: {
              pname = "pegasus-frontend-padmap";
              patches = (old.patches or [ ]) ++ [ ./pegasus/0001-padmap-api.patch ];
            });
          in
          pkgs.writeShellApplication {
            name = "pegasus-fe";
            text = ''
              # SDL does not honour the ID_INPUT_JOYSTICK udev rules that
              # `padmap hide` installs -- it classifies devices from evdev
              # capability bits itself, so the physical pads stay visible to
              # Pegasus and their quirks leak through. Measured here: an N64
              # adapter absent from SDL's database gets a *guessed* layout
              # with Accept on raw button 0, and its worn stick rests at 45%
              # deflection against a 0.5 navigation deadzone.
              #
              # This hint is the SDL-level equivalent of the udev rules,
              # restricting Pegasus to padmap's virtual pads (pid.codes
              # 1209:0001). Off by default: with it on and the daemon not
              # republishing, Pegasus has no controller at all and you would
              # need a keyboard to get to the setup screen.
              if [ "''${PADMAP_ONLY_VIRTUAL:-0}" = "1" ]; then
                export SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT="0x1209/0x0001"
              fi

              # Point the installed theme at this build.
              #
              # The theme is QML read from ~/.config at startup, so it goes
              # stale exactly the way the daemon and padmap-play did: the
              # link was made once, by hand, and every rebuild after that
              # changed the store path without changing where it pointed. A
              # theme fix would then be live in the repo, present in the
              # build, and simply not running -- with the symptom unchanged,
              # which is a genuinely hard thing to see.
              #
              # Only ever replaces a symlink or nothing. If a real directory
              # is there, someone put their own theme in it and it is not
              # ours to overwrite.
              themes="''${XDG_CONFIG_HOME:-$HOME/.config}/pegasus-frontend/themes"
              want="${self.packages.${system}.pegasus-theme}/share/pegasus-frontend/themes/padmap"
              if [ -d "$themes/padmap" ] && [ ! -L "$themes/padmap" ]; then
                echo "padmap: $themes/padmap is a real directory; leaving it" >&2
              elif [ "$(readlink "$themes/padmap" 2>/dev/null)" != "$want" ]; then
                mkdir -p "$themes"
                ln -sfn "$want" "$themes/padmap"
                echo "padmap: theme updated to $want" >&2
              fi

              # The daemon holds the modules it started with, so one left
              # running across a rebuild keeps serving the previous version:
              # still answering, still writing a launch.cfg that looks
              # right, just generated by the old code. Nothing on disk shows
              # it. Checking here, before the front-end that will depend on
              # it starts, is the cheapest place to catch that -- and the
              # daemon restores its assignments on startup, so a restart
              # costs no controller order.
              #
              # Non-fatal: a front-end that refuses to open because a daemon
              # would not start is worse than one with no controllers, since
              # the latter can still be driven by keyboard to fix things.
              if [ "''${PADMAP_SKIP_DAEMON_CHECK:-0}" != "1" ]; then
                ${self.packages.${system}.padmap}/bin/padmap ensure-daemon \
                  || echo "padmap: continuing without a current daemon" >&2
              fi

              # Absolute path, not PATH lookup: this wrapper is also called
              # pegasus-fe and would otherwise be able to re-exec itself.
              exec ${patched}/bin/pegasus-fe "$@"
            '';
          };

        # MAME set name -> real title, for arcade playlists.
        #
        # Pinned to the MAME *2010* XML because that matches the mame2010
        # core the playlists were scanned with; set names drift between MAME
        # versions, so a newer dump resolves fewer of them. Measured: 8215 of
        # 8302 entries (99.0%).
        #
        # The 43MB XML is parsed at build time into a ~1MB JSON table, so the
        # runtime never touches the original.
        packages.mame-titles =
          let
            xml = pkgs.fetchurl {
              url = "https://raw.githubusercontent.com/libretro/libretro-database/"
                + "4b57e60778c9a69459a7587d6cc7e464b3a35ad9/metadat/mame/MAME%202010%20XML.xml";
              hash = "sha256-IOtLCpQEzCsN9kidA3DiFexfPSVcEyH3A334md/e7DY=";
            };
          in
          pkgs.runCommand "padmap-mame-titles"
            { nativeBuildInputs = [ pythonEnv ]; }
            ''
              mkdir -p "$out/share/padmap"
              PYTHONPATH=${./src} python3 -c "
              from pathlib import Path
              from padmap import titles
              table = titles.parse_mame_xml(Path('${xml}'))
              titles.dump_json(table, Path('$out/share/padmap/mame-titles.json'))
              print(f'{len(table)} MAME titles')
              "
            '';

        # Pegasus looks for themes in ~/.config/pegasus-frontend/themes, so
        # this is installed by symlinking rather than by being on PATH.
        packages.pegasus-theme = pkgs.runCommand "padmap-pegasus-theme" { } ''
          mkdir -p "$out/share/pegasus-frontend/themes/padmap"
          cp -r ${./pegasus/theme}/. "$out/share/pegasus-frontend/themes/padmap/"
        '';

        # What a front-end actually spawns to play a game.
        #
        # Pegasus inherits only its own environment, and RetroArch is not
        # installed system-wide here -- it has only ever been on PATH inside
        # padmap's own wrapper. So the launch command has to be an absolute
        # path to something that carries RetroArch with it.
        #
        # Owning --appendconfig here too means the generated metadata does not
        # have to name the config path, and a front-end that knows nothing
        # about padmap still gets the assigned controller order.
        packages.padmap-play = pkgs.writeShellApplication {
          name = "padmap-play";
          runtimeInputs = [ pkgs.retroarch pythonEnv ];
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
            # Python rather than shell. Deciding a console from a core name
            # and a stable key from a ROM path are both table lookups that
            # already exist on the padmap side, and a second copy in shell
            # would be a table with nothing to notice when it fell behind --
            # the failure this project has hit with the launcher path, the
            # theme link and the daemon itself.
            #
            # Never fatal: the default profiles are already on disk, so the
            # worst case is the mapping padmap wrote before scopes existed.
            PYTHONPATH="${./src}''${PYTHONPATH:+:$PYTHONPATH}" \
              python3 -m padmap.launch -- "$@" \
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

            # Only pass the override if it exists: before the first
            # assignment there is no file, and RetroArch treats a missing
            # --appendconfig target as a fatal error.
            if [ -f "$config" ]; then
              retroarch --appendconfig "$config" \
                   ''${args[@]+"''${args[@]}"} "$@" || status=$?
            else
              echo "padmap: no launch config at $config;" \
                   "controller order will be RetroArch's default" >&2
              retroarch "$@" || status=$?
            fi

            exit "$status"
          '';
        };

        # One command for "start the machine": make the daemon current, then
        # hand off to the front-end.
        #
        # Nothing here that `packages.pegasus` does not already do -- the
        # wrapper runs the same check, because Pegasus also gets started by
        # a session manager or a .desktop file, and the check has to happen
        # wherever it is started from. What this adds is a name that says so.
        # PADMAP_SKIP_DAEMON_CHECK stops the two from doing it twice.
        packages.padmap-start = pkgs.writeShellApplication {
          name = "padmap-start";
          text = ''
            # Same escape hatch as the Pegasus wrapper, so the switch means
            # the same thing whichever entry point is used.
            if [ "''${PADMAP_SKIP_DAEMON_CHECK:-0}" != "1" ]; then
              ${self.packages.${system}.padmap}/bin/padmap ensure-daemon \
                || echo "padmap: continuing without a current daemon" >&2
            fi

            # Refresh the udev rules that hide the physical pads from the
            # front-end's own enumeration.
            #
            # They go stale in two ways, and both end in the same baffling
            # symptom. They live in /run/udev/rules.d, which is tmpfs, so a
            # reboot removes them entirely; and they only ever name the
            # controllers that were plugged in when they were written, so any
            # pad bought since is not covered. Either way the front-end sees
            # the *physical* pad as well as padmap's clone, and a wizard that
            # asks the user to press B has that B delivered to the UI as a
            # cancel -- the configuration screen closes on the button it just
            # requested, with nothing anywhere saying why. Diagnosed the hard
            # way from `Gamepad: Connected device 0x0 (Nintendo Switch Pro
            # Controller)` in Pegasus's own lastrun.log.
            #
            # -n so it can never sit at a password prompt on a machine that is
            # plugged into a television. If it cannot elevate, say what to do
            # and carry on: a front-end that starts with stale rules is worse
            # than one that starts, but far better than one that does not.
            if [ "''${PADMAP_SKIP_HIDE:-0}" != "1" ]; then
              if ! sudo -n ${self.packages.${system}.padmap}/bin/padmap hide \
                     >/dev/null 2>&1; then
                echo "padmap: could not refresh the udev hide rules (needs root)." >&2
                echo "padmap: run 'sudo padmap hide' once, or set" >&2
                echo "padmap:   programs.padmap.autoHide = true;" >&2
                echo "padmap: so they are kept current automatically." >&2
              fi
            fi

            # The check has happened (or was deliberately skipped); either
            # way Pegasus should not repeat it.
            export PADMAP_SKIP_DAEMON_CHECK=1
            exec ${self.packages.${system}.pegasus}/bin/pegasus-fe "$@"
          '';
        };

        packages.paddump = pkgs.writeShellApplication {
          name = "paddump";
          runtimeInputs = [ pythonEnv pkgs.udev ];
          text = ''exec python3 ${./tools/paddump.py} "$@"'';
        };

        packages.default = self.packages.${system}.padmap;

        checks.mypy = pkgs.runCommand "padmap-mypy"
          { nativeBuildInputs = [ pythonEnv ]; }
          ''
            cp -r ${./src} src
            export MYPY_CACHE_DIR="$TMPDIR/mypy"
            mypy --config-file ${./mypy.ini} src/padmap
            touch $out
          '';
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
