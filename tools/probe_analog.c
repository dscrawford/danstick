/* Does RetroArch turn a BUTTON bound to an analog half-axis into deflection?
 *
 * danstick maps the N64 C-buttons onto the right analog stick, because that is
 * where mupen64plus-next reads them from. So a user who wants a face button to
 * be C-up gets a profile line like
 *
 *     input_r_y_minus_btn = "3"
 *
 * a button bound to one half of an analog axis. RetroArch's own shipped
 * database does this in 40 profiles (some with hat values, e.g.
 * input_l_x_minus_btn = "h0left"), so the *form* is clearly supported. What
 * that does not tell us is whether the value reaches a core reading
 * RETRO_DEVICE_ANALOG, which is the only way mupen sees a C-button.
 *
 * Reported: mapping Y to C-up "didn't map to the c button". The stored
 * profile, the emitted autoconfig and the button index were all verified
 * correct, so the question is entirely about what RetroArch delivers. Reading
 * its source would be a guess about the build actually installed; this asks
 * the running binary.
 *
 * Prints, once per run:
 *   ANALOG rx=<min>..<max> ry=<min>..<max>   the extremes seen on port 0
 *   JOYPAD <mask>                            RetroPad buttons ever pressed
 * so a harness can inject a press and read back what the core was handed.
 */

#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "libretro.h"

#define PROBE(...) do { printf("ANALOGPROBE: " __VA_ARGS__); fflush(stdout); } while (0)

/* Long enough for a harness to notice the core is up and inject a press,
 * short enough that a hung run is obvious rather than eternal. */
#define PROBE_FRAMES 240

static retro_environment_t   env_cb;
static retro_video_refresh_t video_cb;
static retro_input_poll_t    input_poll_cb;
static retro_input_state_t   input_state_cb;
static retro_audio_sample_t  audio_cb;
static retro_audio_sample_batch_t audio_batch_cb;

static uint16_t framebuf[320 * 240];
static unsigned frames;

/* Extremes rather than a snapshot: the press may land on any frame, and a
 * single sample would report whatever the last one happened to be. */
static int rx_min, rx_max, ry_min, ry_max;
static uint32_t joypad_seen;

void retro_set_environment(retro_environment_t cb)          { env_cb = cb; }
void retro_set_video_refresh(retro_video_refresh_t cb)      { video_cb = cb; }
void retro_set_audio_sample(retro_audio_sample_t cb)        { audio_cb = cb; }
void retro_set_audio_sample_batch(retro_audio_sample_batch_t cb) { audio_batch_cb = cb; }
void retro_set_input_poll(retro_input_poll_t cb)            { input_poll_cb = cb; }
void retro_set_input_state(retro_input_state_t cb)          { input_state_cb = cb; }

void retro_init(void)   { frames = 0; joypad_seen = 0;
                          rx_min = rx_max = ry_min = ry_max = 0; }
void retro_deinit(void) {}
unsigned retro_api_version(void) { return RETRO_API_VERSION; }

void retro_get_system_info(struct retro_system_info *info)
{
   memset(info, 0, sizeof(*info));
   info->library_name     = "danstick analog probe";
   info->library_version  = "1";
   info->valid_extensions = "z64|n64|v64|bin|zip";
   info->need_fullpath    = true;
}

void retro_get_system_av_info(struct retro_system_av_info *info)
{
   memset(info, 0, sizeof(*info));
   info->timing.fps            = 60.0;
   info->timing.sample_rate    = 48000.0;
   info->geometry.base_width   = 320;
   info->geometry.base_height  = 240;
   info->geometry.max_width    = 320;
   info->geometry.max_height   = 240;
   info->geometry.aspect_ratio = 4.0f / 3.0f;
}

void retro_set_controller_port_device(unsigned port, unsigned device)
{
   (void)port; (void)device;
}

void retro_reset(void) {}

static void sample(void)
{
   int rx = input_state_cb(0, RETRO_DEVICE_ANALOG,
                           RETRO_DEVICE_INDEX_ANALOG_RIGHT,
                           RETRO_DEVICE_ID_ANALOG_X);
   int ry = input_state_cb(0, RETRO_DEVICE_ANALOG,
                           RETRO_DEVICE_INDEX_ANALOG_RIGHT,
                           RETRO_DEVICE_ID_ANALOG_Y);
   if (rx < rx_min) rx_min = rx;
   if (rx > rx_max) rx_max = rx;
   if (ry < ry_min) ry_min = ry;
   if (ry > ry_max) ry_max = ry;

   /* The same press read the ordinary way, so a run that sees nothing at all
    * can be told apart from one where only the analog path is empty. */
   for (unsigned id = 0; id <= RETRO_DEVICE_ID_JOYPAD_R3; id++)
      if (input_state_cb(0, RETRO_DEVICE_JOYPAD, 0, id))
         joypad_seen |= (1u << id);
}

void retro_run(void)
{
   input_poll_cb();
   sample();
   video_cb(framebuf, 320, 240, 320 * sizeof(uint16_t));
   if (++frames == PROBE_FRAMES)
   {
      PROBE("ANALOG rx=%d..%d ry=%d..%d\n", rx_min, rx_max, ry_min, ry_max);
      PROBE("JOYPAD 0x%08x\n", joypad_seen);
      PROBE("DONE\n");
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
   enum retro_pixel_format fmt = RETRO_PIXEL_FORMAT_RGB565;
   (void)game;
   env_cb(RETRO_ENVIRONMENT_SET_PIXEL_FORMAT, &fmt);
   PROBE("load_game\n");
   return true;
}

bool retro_load_game_special(unsigned t, const struct retro_game_info *i, size_t n)
{ (void)t; (void)i; (void)n; return false; }
void retro_unload_game(void) {}
unsigned retro_get_region(void) { return RETRO_REGION_NTSC; }
void *retro_get_memory_data(unsigned id) { (void)id; return NULL; }
size_t retro_get_memory_size(unsigned id) { (void)id; return 0; }
