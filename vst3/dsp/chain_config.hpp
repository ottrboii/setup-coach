// Setup Coach Voice — reading chain.json (the app's ProcessingChain) and
// watching it for changes on a background thread.
#pragma once

#include "voice_chain.hpp"

#include <nlohmann/json.hpp>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <mutex>
#include <optional>
#include <sstream>
#include <string>
#include <thread>

namespace setupcoach {

namespace fs = std::filesystem;

/// Parse the JSON written by the app (serde of coach_core ProcessingChain).
/// Only the fields this plugin needs are read; everything else is ignored.
/// Returns nullopt for anything invalid, which means pass-through.
inline std::optional<ChainConfig> parseChainJson(const std::string& text) {
    try {
        auto j = nlohmann::json::parse(text);
        if (!j.is_object()) return std::nullopt;
        ChainConfig c;
        c.monoDownmix = j.value("mono_downmix", false);
        if (!j.contains("highpass_hz") || !j["highpass_hz"].is_number()) return std::nullopt;
        c.highpassHz = std::clamp(j["highpass_hz"].get<double>(), 10.0, 1000.0);
        c.highpassSlopeDbOct = j.value("highpass_slope_db_oct", 12) >= 24 ? 24 : 12;
        if (j.contains("eq") && j["eq"].is_array()) {
            for (const auto& b : j["eq"]) {
                if (c.eq.size() >= kMaxEqBands) break;
                if (!b.is_object()) return std::nullopt;
                EqBandConfig band;
                std::string kind = b.value("kind", std::string("peak"));
                if (kind == "peak") band.kind = EqKind::Peak;
                else if (kind == "low_shelf") band.kind = EqKind::LowShelf;
                else if (kind == "high_shelf") band.kind = EqKind::HighShelf;
                else return std::nullopt;
                band.freqHz = std::clamp(b.value("freq_hz", 1000.0), 20.0, 20000.0);
                band.gainDb = std::clamp(b.value("gain_db", 0.0), -12.0, 12.0);
                band.q = std::clamp(b.value("q", 1.0), 0.1, 10.0);
                c.eq.push_back(band);
            }
        }
        if (j.contains("deesser") && j["deesser"].is_object()) {
            const auto& d = j["deesser"];
            // freq_hz is the centre of the 'ess' band (the core composer puts it on
            // the measured "s" peak, 4.5-9 kHz). Missing, null or non-numeric
            // values keep the fixed defaults, so older chain.json files behave as before.
            auto numOr = [&d](const char* key, double def) {
                auto it = d.find(key);
                return (it != d.end() && it->is_number()) ? it->get<double>() : def;
            };
            DeEsserConfig de;
            de.freqHz = std::clamp(numOr("freq_hz", kDefaultDeEsserHz), 2000.0, 12000.0);
            de.thresholdDb = std::clamp(numOr("threshold_db", -8.0), -40.0, 0.0);
            de.maxReductionDb = std::clamp(numOr("max_reduction_db", 4.0), 0.0, 12.0);
            c.deesser = de;
        }
        return c;
    } catch (...) {
        return std::nullopt;
    }
}

/// Where the app writes chain.json.
///   SETUP_COACH_CHAIN (override, used by tests)
///   Windows: %APPDATA%\SetupCoach\chain.json
///   elsewhere: $XDG_CONFIG_HOME/SetupCoach/chain.json or ~/.config/SetupCoach/chain.json
inline fs::path defaultChainPath() {
    if (const char* p = std::getenv("SETUP_COACH_CHAIN"); p && *p) return fs::path(p);
#ifdef _WIN32
    if (const char* a = std::getenv("APPDATA"); a && *a) return fs::path(a) / "SetupCoach" / "chain.json";
    return fs::path("SetupCoach") / "chain.json";
#else
    if (const char* x = std::getenv("XDG_CONFIG_HOME"); x && *x) return fs::path(x) / "SetupCoach" / "chain.json";
    if (const char* h = std::getenv("HOME"); h && *h) return fs::path(h) / ".config" / "SetupCoach" / "chain.json";
    return fs::path("SetupCoach") / "chain.json";
#endif
}

inline std::optional<ChainConfig> loadChainFile(const fs::path& path) {
    std::ifstream f(path, std::ios::binary);
    if (!f) return std::nullopt;
    std::stringstream ss;
    ss << f.rdbuf();
    return parseChainJson(ss.str());
}

/// Polls chain.json (default 2×/s) and publishes changes to a VoiceChain.
/// Missing or invalid file → pass-through. Never touches the audio thread.
class ChainWatcher {
public:
    explicit ChainWatcher(VoiceChain& chain, fs::path path = defaultChainPath(),
                          std::chrono::milliseconds interval = std::chrono::milliseconds(500))
        : chain_(chain), path_(std::move(path)), interval_(interval) {}

    ~ChainWatcher() { stop(); }

    /// Load once synchronously, then keep watching on a background thread.
    void start() {
        if (thread_.joinable()) return;
        pollOnce();
        stopping_ = false;
        thread_ = std::thread([this] {
            std::unique_lock<std::mutex> lock(m_);
            while (!stopping_) {
                cv_.wait_for(lock, interval_, [this] { return stopping_; });
                if (stopping_) break;
                lock.unlock();
                pollOnce();
                lock.lock();
            }
        });
    }

    void stop() {
        {
            std::lock_guard<std::mutex> lock(m_);
            stopping_ = true;
        }
        cv_.notify_all();
        if (thread_.joinable()) thread_.join();
    }

    /// Check the file now; publishes only when its timestamp/size changed.
    /// Returns true if a new state was published.
    bool pollOnce() {
        std::error_code ec;
        bool exists = fs::exists(path_, ec) && !ec;
        Stamp s;
        if (exists) {
            s.exists = true;
            s.mtime = fs::last_write_time(path_, ec);
            s.size = ec ? 0 : fs::file_size(path_, ec);
        }
        chain_.collectGarbage();
        if (initialized_ && s == stamp_) return false;
        initialized_ = true;
        stamp_ = s;
        chain_.setConfig(exists ? loadChainFile(path_) : std::nullopt);
        return true;
    }

    const fs::path& path() const { return path_; }

private:
    struct Stamp {
        bool exists = false;
        fs::file_time_type mtime{};
        std::uintmax_t size = 0;
        bool operator==(const Stamp& o) const { return exists == o.exists && mtime == o.mtime && size == o.size; }
    };

    VoiceChain& chain_;
    fs::path path_;
    std::chrono::milliseconds interval_;
    std::thread thread_;
    std::mutex m_;
    std::condition_variable cv_;
    bool stopping_ = false;
    bool initialized_ = false;
    Stamp stamp_;
};

}  // namespace setupcoach
