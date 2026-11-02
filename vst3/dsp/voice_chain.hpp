// Setup Coach Voice — real-time DSP (header-only, C++17).
//
// Mirrors coach_core::audio::process::render for the stages OBS lacks:
//   mono downmix → high-pass (12/24 dB/oct) → EQ (≤ 5 bands) → split-band de-esser.
// Gain, expander, compressor and limiter stay in OBS's built-in filters.
//
// Threading model:
//   * Non-real-time threads (config watcher, host setup calls) build fully
//     prepared DspState objects and publish them with VoiceChain::setConfig().
//   * The audio thread picks them up lock-free in process*(), crossfades from the
//     old state to the new one (~20 ms), and hands retired states back through
//     an SPSC ring that the non-RT side frees in collectGarbage().
//   * No allocation, locking or file I/O happens on the audio thread.
#pragma once

#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstddef>
#include <memory>
#include <mutex>
#include <optional>
#include <vector>

namespace setupcoach {

constexpr double kPi = 3.14159265358979323846;
constexpr double kInvSqrt2 = 0.70710678118654752440;

inline double fromDb(double db) { return std::pow(10.0, db / 20.0); }

// ---------------------------------------------------------------------------
// Biquad (RBJ cookbook, transposed direct form II) — identical formulas to
// coach_core::audio::dsp::Biquad.
// ---------------------------------------------------------------------------
class Biquad {
public:
    Biquad() = default;

    static Biquad highpass(double sr, double f, double q) {
        auto [c, s] = w0(sr, f);
        double alpha = s / (2.0 * q);
        return raw((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    }
    static Biquad lowpass(double sr, double f, double q) {
        auto [c, s] = w0(sr, f);
        double alpha = s / (2.0 * q);
        return raw((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    }
    /// Band-pass with 0 dB peak gain.
    static Biquad bandpass(double sr, double f, double q) {
        auto [c, s] = w0(sr, f);
        double alpha = s / (2.0 * q);
        return raw(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    }
    static Biquad peaking(double sr, double f, double q, double gainDb) {
        auto [c, s] = w0(sr, f);
        double a = std::pow(10.0, gainDb / 40.0);
        double alpha = s / (2.0 * q);
        return raw(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a);
    }
    static Biquad lowShelf(double sr, double f, double gainDb, double q = kInvSqrt2) {
        auto [c, s] = w0(sr, f);
        double a = std::pow(10.0, gainDb / 40.0);
        double alpha = s / (2.0 * q);
        double sa = 2.0 * std::sqrt(a) * alpha;
        return raw(a * ((a + 1.0) - (a - 1.0) * c + sa), 2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                   a * ((a + 1.0) - (a - 1.0) * c - sa), (a + 1.0) + (a - 1.0) * c + sa,
                   -2.0 * ((a - 1.0) + (a + 1.0) * c), (a + 1.0) + (a - 1.0) * c - sa);
    }
    static Biquad highShelf(double sr, double f, double gainDb, double q = kInvSqrt2) {
        auto [c, s] = w0(sr, f);
        double a = std::pow(10.0, gainDb / 40.0);
        double alpha = s / (2.0 * q);
        double sa = 2.0 * std::sqrt(a) * alpha;
        return raw(a * ((a + 1.0) + (a - 1.0) * c + sa), -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                   a * ((a + 1.0) + (a - 1.0) * c - sa), (a + 1.0) - (a - 1.0) * c + sa,
                   2.0 * ((a - 1.0) - (a + 1.0) * c), (a + 1.0) - (a - 1.0) * c - sa);
    }

    inline double process(double x) {
        double y = b0_ * x + z1_;
        z1_ = b1_ * x - a1_ * y + z2_;
        z2_ = b2_ * x - a2_ * y;
        return y;
    }

    void reset() { z1_ = z2_ = 0.0; }

    /// Magnitude response (dB) at frequency f.
    double responseDb(double sr, double f) const {
        double w = 2.0 * kPi * f / sr;
        // z^-1 = e^{-jw}
        double c1 = std::cos(w), s1 = -std::sin(w);
        double c2 = std::cos(2 * w), s2 = -std::sin(2 * w);
        double nr = b0_ + b1_ * c1 + b2_ * c2, ni = b1_ * s1 + b2_ * s2;
        double dr = 1.0 + a1_ * c1 + a2_ * c2, di = a1_ * s1 + a2_ * s2;
        double mag = std::sqrt((nr * nr + ni * ni) / (dr * dr + di * di));
        return 20.0 * std::log10(std::max(mag, 1e-12));
    }

private:
    static std::pair<double, double> w0(double sr, double f) {
        f = std::clamp(f, 1.0, sr * 0.49);
        double w = 2.0 * kPi * f / sr;
        return {std::cos(w), std::sin(w)};
    }
    static Biquad raw(double b0, double b1, double b2, double a0, double a1, double a2) {
        Biquad q;
        q.b0_ = b0 / a0;
        q.b1_ = b1 / a0;
        q.b2_ = b2 / a0;
        q.a1_ = a1 / a0;
        q.a2_ = a2 / a0;
        return q;
    }
    double b0_ = 1.0, b1_ = 0.0, b2_ = 0.0, a1_ = 0.0, a2_ = 0.0;
    double z1_ = 0.0, z2_ = 0.0;
};

// ---------------------------------------------------------------------------
// Configuration (subset of coach_core ProcessingChain)
// ---------------------------------------------------------------------------
enum class EqKind { Peak, LowShelf, HighShelf };

struct EqBandConfig {
    EqKind kind = EqKind::Peak;
    double freqHz = 1000.0;
    double gainDb = 0.0;
    double q = 1.0;
};

struct DeEsserConfig {
    double freqHz = 6500.0;
    /// Share (dB) of total energy in the 'ess' band above which highs are reduced.
    double thresholdDb = -8.0;
    double maxReductionDb = 4.0;
};

struct ChainConfig {
    bool monoDownmix = false;
    double highpassHz = 80.0;
    int highpassSlopeDbOct = 12;
    std::vector<EqBandConfig> eq;  // at most kMaxEqBands are used
    std::optional<DeEsserConfig> deesser;
};

constexpr std::size_t kMaxEqBands = 5;

// ---------------------------------------------------------------------------
// Prepared, sample-rate-specific processing state for one configuration.
// A null config builds the identity (pass-through) state.
// ---------------------------------------------------------------------------
class DspState {
public:
    DspState(const ChainConfig* cfg, double sampleRate) : sr_(sampleRate) {
        if (!cfg) {
            identity_ = true;
            return;
        }
        monoDownmix_ = cfg->monoDownmix;
        filters_[numFilters_++] = Biquad::highpass(sr_, cfg->highpassHz, kInvSqrt2);
        if (cfg->highpassSlopeDbOct >= 24)
            filters_[numFilters_++] = Biquad::highpass(sr_, cfg->highpassHz, kInvSqrt2);
        std::size_t bands = std::min(cfg->eq.size(), kMaxEqBands);
        for (std::size_t i = 0; i < bands; ++i) {
            const auto& b = cfg->eq[i];
            switch (b.kind) {
                case EqKind::Peak: filters_[numFilters_++] = Biquad::peaking(sr_, b.freqHz, b.q, b.gainDb); break;
                case EqKind::LowShelf: filters_[numFilters_++] = Biquad::lowShelf(sr_, b.freqHz, b.gainDb); break;
                case EqKind::HighShelf: filters_[numFilters_++] = Biquad::highShelf(sr_, b.freqHz, b.gainDb); break;
            }
        }
        if (cfg->deesser) {
            hasDeesser_ = true;
            de_ = *cfg->deesser;
            deLow_ = Biquad::lowpass(sr_, de_.freqHz * 0.7, kInvSqrt2);
            deSide_ = Biquad::bandpass(sr_, de_.freqHz, 1.0);
            // Envelope coefficients chosen so 48 kHz matches the Rust renderer
            // exactly (0.98 / 0.995 per sample) and other rates keep the same
            // time constants (~1.03 ms band, ~4.16 ms full-band).
            sideCoef_ = std::exp(std::log(0.98) * 48000.0 / sr_);
            fullCoef_ = std::exp(std::log(0.995) * 48000.0 / sr_);
        }
    }

    bool identity() const { return identity_; }
    bool monoDownmix() const { return monoDownmix_; }

    inline double process(double x) {
        if (identity_) return x;
        for (std::size_t i = 0; i < numFilters_; ++i) x = filters_[i].process(x);
        if (hasDeesser_) {
            double sb = deSide_.process(x);
            envS_ = sideCoef_ * envS_ + (1.0 - sideCoef_) * sb * sb;
            envFull_ = fullCoef_ * envFull_ + (1.0 - fullCoef_) * x * x;
            double share = 10.0 * std::log10(std::max(envS_, 1e-18) / std::max(envFull_, 1e-18));
            double hf = x - deLow_.process(x);
            if (share > de_.thresholdDb) {
                double g = fromDb(-std::min((share - de_.thresholdDb) * 2.0, de_.maxReductionDb));
                x += (g - 1.0) * hf;
            }
        }
        return x;
    }

private:
    double sr_;
    bool identity_ = false;
    bool monoDownmix_ = false;
    std::array<Biquad, 2 + kMaxEqBands> filters_{};
    std::size_t numFilters_ = 0;
    bool hasDeesser_ = false;
    DeEsserConfig de_{};
    Biquad deLow_{}, deSide_{};
    double sideCoef_ = 0.98, fullCoef_ = 0.995;
    double envS_ = 0.0, envFull_ = 0.0;
};

// ---------------------------------------------------------------------------
// Tiny lock-free single-producer/single-consumer pointer ring.
// ---------------------------------------------------------------------------
template <typename T, std::size_t N>
class SpscPtrRing {
public:
    bool push(T* p) {
        auto h = head_.load(std::memory_order_relaxed);
        auto next = (h + 1) % N;
        if (next == tail_.load(std::memory_order_acquire)) return false;
        slots_[h] = p;
        head_.store(next, std::memory_order_release);
        return true;
    }
    T* pop() {
        auto t = tail_.load(std::memory_order_relaxed);
        if (t == head_.load(std::memory_order_acquire)) return nullptr;
        T* p = slots_[t];
        tail_.store((t + 1) % N, std::memory_order_release);
        return p;
    }

private:
    std::array<T*, N> slots_{};
    std::atomic<std::size_t> head_{0}, tail_{0};
};

// ---------------------------------------------------------------------------
// VoiceChain: downmix + current/next DspState with crossfades.
// ---------------------------------------------------------------------------
class VoiceChain {
public:
    static constexpr double kFadeMs = 20.0;
    static constexpr int kMaxChannels = 8;

    VoiceChain() { current_ = new DspState(nullptr, sampleRate_); }

    ~VoiceChain() {
        delete current_;
        delete fadingOut_;
        delete pending_.exchange(nullptr);
        collectGarbage();
    }

    VoiceChain(const VoiceChain&) = delete;
    VoiceChain& operator=(const VoiceChain&) = delete;

    // --- Non-real-time API -------------------------------------------------

    /// Call while audio processing is stopped (VST3 setupProcessing/setActive).
    void prepare(double sampleRate, int maxBlockSize) {
        std::lock_guard<std::mutex> lock(configMutex_);
        sampleRate_ = sampleRate;
        scratch_.assign(static_cast<std::size_t>(std::max(maxBlockSize, 1)), 0.0f);
        delete pending_.exchange(nullptr);
        delete fadingOut_;
        fadingOut_ = nullptr;
        delete current_;
        current_ = new DspState(lastConfig_ ? &*lastConfig_ : nullptr, sampleRate_);
        fadePos_ = fadeLen();
        wet_ = targetWet();
        std::fill(std::begin(chanEnergy_), std::end(chanEnergy_), 0.0);
        std::fill(std::begin(chanWeight_), std::end(chanWeight_), 0.0);
        collectGarbage();
    }

    /// Publish a new configuration (nullopt = pass-through). Thread-safe;
    /// call from any non-real-time thread.
    void setConfig(std::optional<ChainConfig> cfg) {
        std::lock_guard<std::mutex> lock(configMutex_);
        lastConfig_ = std::move(cfg);
        hasConfig_.store(lastConfig_.has_value(), std::memory_order_release);
        auto* next = new DspState(lastConfig_ ? &*lastConfig_ : nullptr, sampleRate_);
        // A pending state that the audio thread hasn't taken yet was never used
        // by it, so it is safe to free here.
        delete pending_.exchange(next, std::memory_order_acq_rel);
        collectGarbage();
    }

    bool hasConfig() const { return hasConfig_.load(std::memory_order_acquire); }

    /// Free states retired by the audio thread. Non-real-time only.
    void collectGarbage() {
        std::lock_guard<std::mutex> lock(gcMutex_);  // single consumer at a time
        while (DspState* p = retired_.pop()) delete p;
    }

    // --- Real-time API -----------------------------------------------------

    /// Host bypass (VST3 'Bypass' parameter). Crossfaded.
    void setHostBypass(bool b) { hostBypass_ = b; }

    /// Downmix + process into a mono buffer. `in` holds `nch` channel pointers.
    void process(const float* const* in, int nch, float* outMono, int n) {
        takePending();
        downmix(in, nch, outMono, n);
        const int fl = fadeLen();
        for (int i = 0; i < n; ++i) {
            double x = outMono[i];
            double y = current_->process(x);
            if (fadingOut_) {
                double t = static_cast<double>(fadePos_) / fl;
                y = t * y + (1.0 - t) * fadingOut_->process(x);
                if (++fadePos_ >= fl) {
                    if (!retired_.push(fadingOut_)) { /* ring full: leak rather than block */ }
                    fadingOut_ = nullptr;
                }
            }
            outMono[i] = static_cast<float>(y);
        }
    }

    /// Full plugin path: process to mono, then write every output channel as a
    /// crossfade between the dry input channel and the processed mono signal.
    /// With no config (or host bypass) the output equals the input exactly.
    void processToOutputs(const float* const* in, int nchIn, float* const* out, int nchOut, int n) {
        if (n <= 0 || nchOut <= 0) return;
        if (static_cast<int>(scratch_.size()) < n) {
            // Host exceeded maxBlockSize: process in chunks (no allocation).
            int done = 0;
            const float* inPtrs[kMaxChannels];
            float* outPtrs[kMaxChannels];
            while (done < n) {
                int len = std::min(n - done, static_cast<int>(scratch_.size()));
                for (int c = 0; c < std::min(nchIn, kMaxChannels); ++c) inPtrs[c] = in[c] + done;
                for (int c = 0; c < std::min(nchOut, kMaxChannels); ++c) outPtrs[c] = out[c] + done;
                processToOutputs(inPtrs, std::min(nchIn, kMaxChannels), outPtrs, std::min(nchOut, kMaxChannels), len);
                done += len;
            }
            return;
        }
        float* mono = scratch_.data();
        if (nchIn > 0) {
            process(in, nchIn, mono, n);
        } else {
            std::fill(mono, mono + n, 0.0f);
        }
        const double target = targetWet();
        const double step = 1.0 / fadeLen();
        for (int i = 0; i < n; ++i) {
            if (wet_ < target) wet_ = std::min(target, wet_ + step);
            else if (wet_ > target) wet_ = std::max(target, wet_ - step);
            for (int c = 0; c < nchOut; ++c) {
                double dry = nchIn > 0 ? in[std::min(c, nchIn - 1)][i] : 0.0;
                out[c][i] = static_cast<float>((1.0 - wet_) * dry + wet_ * mono[i]);
            }
        }
    }

    double sampleRate() const { return sampleRate_; }

private:
    int fadeLen() const { return std::max(1, static_cast<int>(sampleRate_ * kFadeMs / 1000.0)); }

    double targetWet() const {
        return (!hostBypass_ && current_ && !current_->identity()) ? 1.0 : 0.0;
    }

    void takePending() {
        if (fadingOut_) return;  // finish the running fade first
        DspState* next = pending_.exchange(nullptr, std::memory_order_acq_rel);
        if (!next) return;
        fadingOut_ = current_;
        current_ = next;
        fadePos_ = 0;
    }

    // Mono downmix. With monoDownmix, channels more than 30 dB below the
    // loudest (e.g. interface input 1 arriving as left-only stereo) are
    // dropped instead of averaged in, so the voice keeps its full level.
    void downmix(const float* const* in, int nch, float* out, int n) {
        nch = std::min(nch, kMaxChannels);
        if (nch == 1) {
            std::copy(in[0], in[0] + n, out);
            return;
        }
        double newWeight[kMaxChannels] = {};
        bool pickLive = current_->monoDownmix();
        if (pickLive) {
            // Block energy, smoothed with ~300 ms time constant.
            double coef = std::exp(-static_cast<double>(n) / (0.3 * sampleRate_));
            double maxE = 0.0;
            for (int c = 0; c < nch; ++c) {
                double e = 0.0;
                for (int i = 0; i < n; ++i) e += static_cast<double>(in[c][i]) * in[c][i];
                e /= std::max(n, 1);
                chanEnergy_[c] = coef * chanEnergy_[c] + (1.0 - coef) * e;
                maxE = std::max(maxE, chanEnergy_[c]);
            }
            int live = 0;
            for (int c = 0; c < nch; ++c) {
                bool isLive = maxE <= 0.0 || chanEnergy_[c] >= maxE * 1e-3;  // within 30 dB
                newWeight[c] = isLive ? 1.0 : 0.0;
                live += isLive ? 1 : 0;
            }
            for (int c = 0; c < nch; ++c) newWeight[c] /= std::max(live, 1);
        } else {
            for (int c = 0; c < nch; ++c) newWeight[c] = 1.0 / nch;
        }
        // Ramp weights across the block to avoid clicks when they change.
        for (int i = 0; i < n; ++i) {
            double t = static_cast<double>(i + 1) / n;
            double acc = 0.0;
            for (int c = 0; c < nch; ++c) {
                double w = chanWeight_[c] + (newWeight[c] - chanWeight_[c]) * t;
                acc += w * in[c][i];
            }
            out[i] = static_cast<float>(acc);
        }
        for (int c = 0; c < nch; ++c) chanWeight_[c] = newWeight[c];
    }

    // Config side (guarded by configMutex_).
    std::mutex configMutex_;
    std::mutex gcMutex_;
    std::optional<ChainConfig> lastConfig_;
    std::atomic<bool> hasConfig_{false};

    // Shared.
    double sampleRate_ = 48000.0;
    std::atomic<DspState*> pending_{nullptr};
    SpscPtrRing<DspState, 16> retired_;

    // Audio side.
    DspState* current_ = nullptr;
    DspState* fadingOut_ = nullptr;
    int fadePos_ = 0;
    bool hostBypass_ = false;
    double wet_ = 0.0;
    std::vector<float> scratch_ = std::vector<float>(4096, 0.0f);
    double chanEnergy_[kMaxChannels] = {};
    double chanWeight_[kMaxChannels] = {};
};

}  // namespace setupcoach
