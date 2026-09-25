/* Minimal libretro core that declares 4 controller ports and logs every
 * retro_set_controller_port_device call RetroArch makes.
 *
 * Written because there is no other way to see which core ports RetroArch
 * actually populates. mupen64plus's "Game controller N (Standard controller)"
 * lines are printed inside retro_load_game, before CMD_EVENT_CONTROLLER_INIT
 * runs, so they do not change even with -N 1; and the "[Input] Input device ID
 * %u is unknown" warning never fires for a bogus id, because
 * libretro_find_controller_description returns NULL and the warning is guarded
 * on a non-NULL desc.
 *
 * Build and run (needs a real X display -- the `null` video driver aborts with
 * "Cannot initialize input driver", so use Xvfb):
 *
 *   SRC=$(nix build --no-link --print-out-paths 'nixpkgs#retroarch-bare.src')
 *   gcc -shared -fPIC -O1 -o probe.so tools/probe_libretro.c \
 *       -I "$SRC/libretro-common/include"
 *   Xvfb :78 -screen 0 640x480x24 & sleep 2
 *   DISPLAY=:78 timeout 25 danstick-play -L ./probe.so any.z64 2>&1 | grep PROBE:
 *
 * Expected with one assigned player:
 *
 *   PROBE: set_controller_port_device(port=0, device=1) -> JOYPAD
 *   PROBE: set_controller_port_device(port=1, device=0) -> NONE
 *   PROBE: set_controller_port_device(port=2, device=0) -> NONE
 *   PROBE: set_controller_port_device(port=3, device=0) -> NONE
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "libretro.h"

static retro_environment_t env_cb;
static retro_video_refresh_t video_cb;
static retro_audio_sample_batch_t audio_batch_cb;
static retro_input_poll_t input_poll_cb;
static retro_input_state_t input_state_cb;
static retro_audio_sample_t audio_cb;

static uint16_t framebuf[320 * 240];
static unsigned frames;

/* Device type last set on each core port, so the run can end with one line
 * that is trivial to assert on. -1 means "never set". */
#define MAX_PROBE_PORTS 16
static int port_device[MAX_PROBE_PORTS];

#define PROBE(...) do { fprintf(stderr, "PROBE: " __VA_ARGS__); fflush(stderr); } while (0)

/* Frames to run before asking RetroArch to quit. Long enough that
 * command_event_init_controllers has certainly run, short enough that a test
 * does not need a generous timeout. Self-shutdown matters because a front-end
 * that spawned us stays suspended until the child exits. */
#define PROBE_FRAMES 180

static void probe_report(void)
{
   char line[256];
   size_t len = 0;
   int i;
   for (i = 0; i < 4; i++)
      len += snprintf(line + len, sizeof(line) - len, "%s%d",
                      i ? "," : "", port_device[i]);
   PROBE("SUMMARY ports0-3=%s (1=JOYPAD 0=NONE -1=unset)\n", line);
}

void retro_set_environment(retro_environment_t cb)
{
   bool no_content = true;
   env_cb = cb;
   cb(RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME, &no_content);
}

void retro_set_video_refresh(retro_video_refresh_t cb)      { video_cb = cb; }
void retro_set_audio_sample(retro_audio_sample_t cb)        { audio_cb = cb; }
void retro_set_audio_sample_batch(retro_audio_sample_batch_t cb) { audio_batch_cb = cb; }
void retro_set_input_poll(retro_input_poll_t cb)            { input_poll_cb = cb; }
void retro_set_input_state(retro_input_state_t cb)          { input_state_cb = cb; }

void retro_init(void)
{
   int i;
   frames = 0;
   for (i = 0; i < MAX_PROBE_PORTS; i++)
      port_device[i] = -1;
}
void retro_deinit(void) {}
unsigned retro_api_version(void) { return RETRO_API_VERSION; }

void retro_get_system_info(struct retro_system_info *info)
{
   memset(info, 0, sizeof(*info));
   info->library_name     = "PortProbe";
   info->library_version  = "1";
   info->need_fullpath    = false;
   info->valid_extensions = "z64|n64|v64|probe";
}

void retro_get_system_av_info(struct retro_system_av_info *info)
{
   memset(info, 0, sizeof(*info));
   info->timing.fps            = 60.0;
   info->timing.sample_rate    = 44100.0;
   info->geometry.base_width   = 320;
   info->geometry.base_height  = 240;
   info->geometry.max_width    = 320;
   info->geometry.max_height   = 240;
   info->geometry.aspect_ratio = 4.0f / 3.0f;
}

/* This is the whole point of the core. */
void retro_set_controller_port_device(unsigned port, unsigned device)
{
   const char *name = "?";
   switch (device)
   {
      case RETRO_DEVICE_NONE:   name = "NONE";   break;
      case RETRO_DEVICE_JOYPAD: name = "JOYPAD"; break;
      case RETRO_DEVICE_ANALOG: name = "ANALOG"; break;
   }
   if (port < MAX_PROBE_PORTS)
      port_device[port] = (int)device;
   PROBE("set_controller_port_device(port=%u, device=%u) -> %s\n",
         port, device, name);
}

void retro_reset(void) {}

void retro_run(void)
{
   input_poll_cb();
   video_cb(framebuf, 320, 240, 320 * sizeof(uint16_t));
   if (++frames == PROBE_FRAMES)
   {
      probe_report();
      /* Quit rather than run forever: a front-end that spawned us is
       * suspended until we exit, so a test driving Pegasus would hang. */
      env_cb(RETRO_ENVIRONMENT_SHUTDOWN, NULL);
   }
}

size_t retro_serialize_size(void) { return 0; }
bool retro_serialize(void *d, size_t s) { (void)d; (void)s; return false; }
bool retro_unserialize(const void *d, size_t s) { (void)d; (void)s; return false; }
void retro_cheat_reset(void) {}
void retro_cheat_set(unsigned i, bool e, const char *c) { (void)i; (void)e; (void)c; }

bool retro_load_game(const struct retro_game_info *game)
{
   /* Four ports, exactly like every N64 core, so sys_info->ports.size == 4
    * and command_event_init_controllers iterates all four. */
   static const struct retro_controller_description port_desc[] = {
      { "N64 Controller", RETRO_DEVICE_JOYPAD },
      { NULL, 0 },
   };
   static const struct retro_controller_info ports[] = {
      { port_desc, 1 }, { port_desc, 1 }, { port_desc, 1 }, { port_desc, 1 },
      { NULL, 0 },
   };
   enum retro_pixel_format fmt = RETRO_PIXEL_FORMAT_RGB565;

   (void)game;
   env_cb(RETRO_ENVIRONMENT_SET_PIXEL_FORMAT, &fmt);
   env_cb(RETRO_ENVIRONMENT_SET_CONTROLLER_INFO, (void*)ports);
   PROBE("load_game: declared 4 controller ports\n");
   return true;
}

bool retro_load_game_special(unsigned t, const struct retro_game_info *i, size_t n)
{ (void)t; (void)i; (void)n; return false; }
void retro_unload_game(void) {}
unsigned retro_get_region(void) { return RETRO_REGION_NTSC; }
void *retro_get_memory_data(unsigned id) { (void)id; return NULL; }
size_t retro_get_memory_size(unsigned id) { (void)id; return 0; }
