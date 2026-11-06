#pragma once

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct PedalNamModel PedalNamModel;

enum PedalNamStatus
{
  PEDAL_NAM_OK = 0,
  PEDAL_NAM_INVALID_ARGUMENT = 1,
  PEDAL_NAM_LOAD_FAILED = 2,
  PEDAL_NAM_UNSUPPORTED_LAYOUT = 3,
  PEDAL_NAM_SAMPLE_RATE_MISMATCH = 4,
  PEDAL_NAM_PROCESS_FAILED = 5
};

int pedal_nam_load(const char* path, double sample_rate, int max_buffer_frames, int full_size, PedalNamModel** output,
                   char* error_message, size_t error_message_capacity);
void pedal_nam_free(PedalNamModel* model);
int pedal_nam_process(PedalNamModel* model, const float* input, float* output, int frames);
double pedal_nam_expected_sample_rate(const PedalNamModel* model);
/* Optional calibration/loudness values return NaN when unavailable. */
double pedal_nam_input_level(const PedalNamModel* model);
double pedal_nam_output_level(const PedalNamModel* model);
double pedal_nam_loudness(const PedalNamModel* model);
int pedal_nam_supports_sizes(const PedalNamModel* model);

#ifdef __cplusplus
}
#endif
