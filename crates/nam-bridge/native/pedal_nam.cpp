#include "pedal_nam.h"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <exception>
#include <filesystem>
#include <memory>
#include <limits>
#include <string>
#include <vector>

#include "NAM/dsp.h"
#include "NAM/get_dsp.h"
#include "NAM/slimmable.h"

struct PedalNamModel
{
  std::unique_ptr<nam::DSP> dsp;
  int max_buffer_frames;
  bool supports_sizes;
};

namespace
{
void set_error(char* destination, const size_t capacity, const std::string& message)
{
  if (destination == nullptr || capacity == 0)
    return;
  const auto count = std::min(capacity - 1, message.size());
  std::memcpy(destination, message.data(), count);
  destination[count] = '\0';
}
} // namespace

int pedal_nam_load(const char* path, const double sample_rate, const int max_buffer_frames, const int full_size, PedalNamModel** output,
                   char* error_message, const size_t error_message_capacity)
{
  if (path == nullptr || output == nullptr || !std::isfinite(sample_rate) || sample_rate <= 0.0 || max_buffer_frames <= 0 || (full_size != 0 && full_size != 1))
  {
    set_error(error_message, error_message_capacity, "invalid NAM load arguments");
    return PEDAL_NAM_INVALID_ARGUMENT;
  }
  *output = nullptr;

  try
  {
    nam::DspLoadOptions options;
    options.prewarm = false;
    auto dsp = nam::get_dsp(std::filesystem::path(path), options);
    if (dsp == nullptr)
    {
      set_error(error_message, error_message_capacity, "NAM Core returned no model");
      return PEDAL_NAM_LOAD_FAILED;
    }
    if (dsp->NumInputChannels() != 1 || dsp->NumOutputChannels() != 1)
    {
      set_error(error_message, error_message_capacity, "V0 supports mono-input, mono-output NAM models only");
      return PEDAL_NAM_UNSUPPORTED_LAYOUT;
    }

    const auto expected_rate = dsp->GetExpectedSampleRate();
    if (expected_rate != NAM_UNKNOWN_EXPECTED_SAMPLE_RATE && std::abs(expected_rate - sample_rate) > 0.5)
    {
      set_error(error_message, error_message_capacity,
                "model sample rate does not match the engine sample rate");
      return PEDAL_NAM_SAMPLE_RATE_MISMATCH;
    }

    dsp->SetPrewarmOnReset(false);
    dsp->Reset(sample_rate, max_buffer_frames);
    // TONE3000's NamEngine is the initialization-order reference (see
    // docs/nam-model-quality.md): configure buffers before selecting a tier.
    // Use endpoints, not 0.5: a container's boundary selects the upper tier.
    auto* slimmable = dynamic_cast<nam::SlimmableModel*>(dsp.get());
    const bool supports_sizes = slimmable && !slimmable->GetSlimmableSizeBreakpoints().empty();
    if (supports_sizes)
      slimmable->SetSlimmableSize(full_size ? 1.0 : 0.0);
    dsp->prewarm();
    if (supports_sizes)
    {
      // SlimmableWavenet may defer adopting its prepared child until process().
      // Drain that handoff here so child destruction never hits the callback.
      std::vector<float> silence(max_buffer_frames, 0.0f);
      std::vector<float> scratch(max_buffer_frames, 0.0f);
      auto* in = silence.data();
      auto* out = scratch.data();
      dsp->process(&in, &out, max_buffer_frames);
    }

    auto model = std::make_unique<PedalNamModel>();
    model->dsp = std::move(dsp);
    model->max_buffer_frames = max_buffer_frames;
    model->supports_sizes = supports_sizes;
    *output = model.release();
    return PEDAL_NAM_OK;
  }
  catch (const std::exception& exception)
  {
    set_error(error_message, error_message_capacity, exception.what());
    return PEDAL_NAM_LOAD_FAILED;
  }
  catch (...)
  {
    set_error(error_message, error_message_capacity, "unknown NAM loading failure");
    return PEDAL_NAM_LOAD_FAILED;
  }
}

void pedal_nam_free(PedalNamModel* model) { delete model; }

int pedal_nam_supports_sizes(const PedalNamModel* model)
{
  return model && model->supports_sizes ? 1 : 0;
}

int pedal_nam_process(PedalNamModel* model, const float* input, float* output, const int frames)
{
  if (model == nullptr || input == nullptr || output == nullptr || frames < 0 || frames > model->max_buffer_frames)
    return PEDAL_NAM_INVALID_ARGUMENT;

  try
  {
    auto* input_pointer = const_cast<float*>(input);
    auto* output_pointer = output;
    model->dsp->process(&input_pointer, &output_pointer, frames);
    return PEDAL_NAM_OK;
  }
  catch (...)
  {
    return PEDAL_NAM_PROCESS_FAILED;
  }
}

double pedal_nam_expected_sample_rate(const PedalNamModel* model)
{
  return model == nullptr ? NAM_UNKNOWN_EXPECTED_SAMPLE_RATE : model->dsp->GetExpectedSampleRate();
}

double pedal_nam_input_level(const PedalNamModel* model)
{
  try { if (model && model->dsp->HasInputLevel()) return model->dsp->GetInputLevel(); } catch (...) {}
  return std::numeric_limits<double>::quiet_NaN();
}

double pedal_nam_output_level(const PedalNamModel* model)
{
  try { if (model && model->dsp->HasOutputLevel()) return model->dsp->GetOutputLevel(); } catch (...) {}
  return std::numeric_limits<double>::quiet_NaN();
}

double pedal_nam_loudness(const PedalNamModel* model)
{
  try { if (model && model->dsp->HasLoudness()) return model->dsp->GetLoudness(); } catch (...) {}
  return std::numeric_limits<double>::quiet_NaN();
}
