# The environment padmap's tests run in on the cluster, not the source: it
# changes with `Cargo.lock`, and `tools/cluster-test` streams the tree in.
{
  pkgs,
  cargoLock,
}:
let
  # The crates, fetched by nix from `Cargo.lock`: a pod builds offline.
  vendored = pkgs.rustPlatform.importCargoLock { lockFile = cargoLock; };

  cargoHome = pkgs.runCommand "padmap-test-cargo-home" { } ''
    mkdir -p $out
    cat > $out/config.toml <<EOF
    [source.crates-io]
    replace-with = "vendored-sources"

    [source.vendored-sources]
    directory = "${vendored}"
    EOF
  '';

  tools = [
    pkgs.cargo
    pkgs.rustc
    pkgs.clippy
    pkgs.rustfmt
    pkgs.stdenv.cc
    pkgs.pkg-config
    pkgs.bubblewrap # `exec` sandboxes a game with it
    pkgs.bashInteractive
    pkgs.coreutils
    pkgs.findutils
    pkgs.gnugrep
    pkgs.gnutar
    pkgs.gzip
    pkgs.procps
    pkgs.util-linux
  ];

  # Linked by padmap-input, and loaded again by every binary a test runs.
  libraries = [
    pkgs.udev
    pkgs.sdl3
  ];
in
pkgs.dockerTools.buildLayeredImage {
  name = "padmap-tests";
  contents = tools ++ [
    pkgs.dockerTools.binSh
    pkgs.dockerTools.usrBinEnv
    pkgs.dockerTools.caCertificates
  ];
  extraCommands = ''
    mkdir -p tmp src
    chmod 1777 tmp
  '';
  config = {
    WorkingDir = "/src/rust";
    Env = [
      "PATH=${pkgs.lib.makeBinPath tools}"
      "CARGO_HOME=${cargoHome}"
      "CARGO_TARGET_DIR=/tmp/target"
      "PKG_CONFIG_PATH=${pkgs.lib.makeSearchPath "lib/pkgconfig" (map pkgs.lib.getDev libraries)}"
      # SDL3 is linked by name, not through pkg-config (sdlprobe.rs), so the
      # linker needs its directory as well as the loader does.
      "LIBRARY_PATH=${pkgs.lib.makeLibraryPath libraries}"
      "LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath libraries}"
      "HOME=/tmp/home"
      # A pod is not a desk: nothing here is anybody's live daemon.
      "XDG_RUNTIME_DIR=/tmp/xdg"
      "SDL_VIDEODRIVER=dummy"
    ];
    Cmd = [
      "sleep"
      "infinity"
    ];
  };
}
