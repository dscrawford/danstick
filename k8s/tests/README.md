# Running danstick's tests on the cluster

The journeys make real pads through `/dev/uinput` and start real daemons
against them, several at once. On a desktop that is load the person using it
feels and fake pads their games can see, so the suite runs in a pod.

```bash
tools/cluster-test                                           # everything
tools/cluster-test -p danstick-core                            # any cargo test arguments
tools/cluster-test -p danstick-rs --test daemon_journey -- exec_reserves
DANSTICK_CLUSTER_LINT=1 tools/cluster-test                     # fmt and clippy there too
```

The exit status is cargo's.

## What happens

1. `nix build .#test-image` (`nix/test-image.nix`): a Rust toolchain, the
   crates `Cargo.lock` names vendored so the pod builds offline, and what
   danstick links and runs -- udev, SDL3, bubblewrap. No danstick source. It only
   changes when the lock file does, so this is a no-op most runs.
2. It is pushed to the cluster registry (`192.168.0.2:30500`) the first time
   its tag is missing, tagged by its store hash so a tag always names one
   image. The pod pulls it as `localhost:30500`, the NodePort name the nodes'
   containerd trusts.
3. A pod from `pod.yaml` starts, the working tree -- uncommitted edits
   included, build output excluded, about a megabyte -- is streamed in with
   tar, and `cargo test --offline` builds and runs there.
4. The pod is deleted when the script ends, interrupted or not. A pod left
   behind anyway ends itself after an hour (`activeDeadlineSeconds`).

## What the pod needs

- **Privileged.** containerd's device cgroup denies `/dev/uinput` to an
  unprivileged container whatever the node's permissions say.
- **The node's `/dev/input`**, where the pads it makes appear.
- **The node's `/run/udev`, read-only.** danstick reads udev's properties --
  `ID_INPUT_KEY` for the desk's keyboards, among others -- and the nodes run
  NixOS with udevd, which describes a pad the pod made exactly as it would on
  a desk. Mounting its database is more faithful than GOTG's
  `SDL_JOYSTICK_DISABLE_UDEV`, which only SDL would honour.

The nodes are servers; nobody is playing on them, so a fake pad there takes
nobody's seat.
