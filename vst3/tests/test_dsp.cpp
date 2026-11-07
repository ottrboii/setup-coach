// Setup Coach Voice — DSP tests (no framework).
#include "../dsp/chain_config.hpp"
#include "../dsp/voice_chain.hpp"

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <functional>
#include <random>
#include <string>
#include <vector>

using namespace setupcoach;

static int g_failures = 0;
static int g_checks = 0;
#define CHECK(cond, ...)                                                   \
    do {                                                                   \
        ++g_checks;                                                        \
        if (!(cond)) {                                                     \
            ++g_failures;                                                  \
            std::fprintf(stderr, "FAIL %s:%d: %s — ", __FILE__, __LINE__, #cond); \
            std::fprintf(stderr, __VA_ARGS__);                             \
            std::fprintf(stderr, "\n");                                    \
        }                                                                  \
    } while (0)

static const double SR = 48000.0;

static std::vector<float> sine(double f, double amp, double secs) {
    std::vector<float> x(static_cast<size_t>(secs * SR));
    for (size_t i = 0; i < x.size(); ++i) x[i] = static_cast<float>(amp * std::sin(2 * kPi * f * i / SR));
    return x;
}

static double rmsDb(const float* x, size_t n) {
    double s = 0;
    for (size_t i = 0; i < n; ++i) s += double(x[i]) * x[i];
    return 10 * std::log10(std::max(s / std::max<size_t>(n, 1), 1e-24));
}

// Steady-state level (skip the first 0.5 s).
static double tailDb(const std::vector<float>& x) {
    size_t skip = static_cast<size_t>(0.5 * SR);
    return rmsDb(x.data() + skip, x.size() - skip);
}

static std::vector<float> runMono(VoiceChain& vc, const std::vector<float>& in) {
    std::vector<float> out(in.size());
    const size_t block = 256;
    for (size_t i = 0; i < in.size(); i += block) {
        int n = static_cast<int>(std::min(block, in.size() - i));
        const float* ptrs[1] = {in.data() + i};
        vc.process(ptrs, 1, out.data() + i, n);
    }
    return out;
}

static void runStereo(VoiceChain& vc, const std::vector<float>& l, const std::vector<float>& r,
                      std::vector<float>& ol, std::vector<float>& orr) {
    ol.assign(l.size(), 0.f);
    orr.assign(l.size(), 0.f);
    const size_t block = 480;
    for (size_t i = 0; i < l.size(); i += block) {
        int n = static_cast<int>(std::min(block, l.size() - i));
        const float* in[2] = {l.data() + i, r.data() + i};
        float* out[2] = {ol.data() + i, orr.data() + i};
        vc.processToOutputs(in, 2, out, 2, n);
    }
}

static ChainConfig baseConfig() {
    ChainConfig c;
    c.highpassHz = 20.0;
    c.highpassSlopeDbOct = 12;
    return c;
}

static VoiceChain& prepared(VoiceChain& vc, std::optional<ChainConfig> c) {
    vc.prepare(SR, 512);
    vc.setConfig(std::move(c));
    return vc;
}

static std::vector<float> hfNoise(double secs, double amp, unsigned seed = 1) {
    std::mt19937 rng(seed);
    std::uniform_real_distribution<double> u(-1, 1);
    Biquad hp1 = Biquad::highpass(SR, 4500, 0.707), hp2 = Biquad::highpass(SR, 4500, 0.707);
    Biquad lp = Biquad::lowpass(SR, 9000, 0.707);
    std::vector<float> x(static_cast<size_t>(secs * SR));
    for (auto& v : x) v = static_cast<float>(amp * lp.process(hp2.process(hp1.process(u(rng)))));
    return x;
}

static void test_highpass() {
    for (int slope : {12, 24}) {
        ChainConfig c = baseConfig();
        c.highpassHz = 100.0;
        c.highpassSlopeDbOct = slope;
        VoiceChain vc;
        prepared(vc, c);
        auto in1k = sine(1000, 0.5, 1.5), in25 = sine(25, 0.5, 1.5);
        double ref = tailDb(in1k);
        double g1k = tailDb(runMono(vc, in1k)) - ref;
        VoiceChain vc2;
        prepared(vc2, c);
        double g25 = tailDb(runMono(vc2, in25)) - tailDb(in25);
        CHECK(std::fabs(g1k) < 0.3, "slope %d: 1 kHz gain %.2f dB", slope, g1k);
        CHECK(g25 < (slope == 24 ? -40.0 : -20.0), "slope %d: 25 Hz gain %.2f dB", slope, g25);
        // Cross-check against the analytic biquad response.
        Biquad hp = Biquad::highpass(SR, 100.0, kInvSqrt2);
        double expected = hp.responseDb(SR, 25.0) * (slope == 24 ? 2 : 1);
        CHECK(std::fabs(g25 - expected) < 0.3, "slope %d: 25 Hz measured %.2f vs analytic %.2f", slope, g25, expected);
    }
}

static void test_eq_bands() {
    struct Case { EqKind kind; double f, gain, q, probe; };
    for (Case cs : {Case{EqKind::Peak, 1000, -4.0, 1.0, 1000}, Case{EqKind::Peak, 3000, 3.0, 0.8, 3000},
                    Case{EqKind::LowShelf, 180, 2.0, 0.7, 60}, Case{EqKind::HighShelf, 8000, 2.0, 0.8, 14000}}) {
        ChainConfig c = baseConfig();
        c.eq.push_back({cs.kind, cs.f, cs.gain, cs.q});
        VoiceChain vc;
        prepared(vc, c);
        auto in = sine(cs.probe, 0.25, 1.5);
        double g = tailDb(runMono(vc, in)) - tailDb(in);
        // Independent expectation: product of the analytic responses.
        Biquad band = cs.kind == EqKind::Peak       ? Biquad::peaking(SR, cs.f, cs.q, cs.gain)
                      : cs.kind == EqKind::LowShelf ? Biquad::lowShelf(SR, cs.f, cs.gain)
                                                    : Biquad::highShelf(SR, cs.f, cs.gain);
        double expected = band.responseDb(SR, cs.probe) + Biquad::highpass(SR, 20.0, kInvSqrt2).responseDb(SR, cs.probe);
        CHECK(std::fabs(g - expected) < 0.1, "eq kind %d @%.0f: measured %.3f expected %.3f", int(cs.kind), cs.probe, g, expected);
        if (cs.kind == EqKind::Peak) CHECK(std::fabs(g - cs.gain) < 0.1, "peak centre gain %.3f vs %.1f", g, cs.gain);
    }
}

static void test_left_only_reaches_both_ears() {
    ChainConfig c = baseConfig();
    c.monoDownmix = true;
    VoiceChain vc;
    prepared(vc, c);
    auto l = sine(440, 0.3, 1.5);
    std::vector<float> r(l.size(), 0.f), ol, orr;
    runStereo(vc, l, r, ol, orr);
    double in = tailDb(l);
    CHECK(std::fabs(tailDb(ol) - in) < 0.5, "left out %.2f vs in %.2f", tailDb(ol), in);
    CHECK(std::fabs(tailDb(orr) - in) < 0.5, "right out %.2f vs in %.2f", tailDb(orr), in);

    // Without mono_downmix the channels are averaged (−6 dB for left-only).
    ChainConfig avg = baseConfig();
    VoiceChain vc2;
    prepared(vc2, avg);
    runStereo(vc2, l, r, ol, orr);
    CHECK(std::fabs(tailDb(orr) - (in - 6.02)) < 0.5, "averaged right %.2f", tailDb(orr));
}

static void test_deesser() {
    ChainConfig with = baseConfig();
    with.deesser = DeEsserConfig{6500.0, -8.0, 6.0};
    ChainConfig without = baseConfig();

    auto ess = hfNoise(1.5, 0.3);
    VoiceChain a, b;
    prepared(a, with);
    prepared(b, without);
    double reduction = tailDb(runMono(b, ess)) - tailDb(runMono(a, ess));
    CHECK(reduction > 4.0, "de-esser reduced 5-9 kHz noise by only %.2f dB", reduction);

    auto tone = sine(1000, 0.3, 1.5);
    VoiceChain c, d;
    prepared(c, with);
    prepared(d, without);
    double diff = std::fabs(tailDb(runMono(c, tone)) - tailDb(runMono(d, tone)));
    CHECK(diff < 0.1, "de-esser changed a 1 kHz tone by %.3f dB", diff);
}

static void test_passthrough_when_file_missing() {
    VoiceChain vc;
    vc.prepare(SR, 512);
    ChainWatcher w(vc, std::filesystem::temp_directory_path() / "setupcoach-does-not-exist" / "chain.json");
    w.pollOnce();
    CHECK(!vc.hasConfig(), "config should be absent");
    auto l = sine(300, 0.4, 0.5), r = sine(700, 0.2, 0.5), ol = l, orr = r;
    runStereo(vc, l, r, ol, orr);
    bool identical = true;
    for (size_t i = 0; i < l.size(); ++i) identical &= (ol[i] == l[i] && orr[i] == r[i]);
    CHECK(identical, "outputs must equal inputs exactly when chain.json is missing");
}

static void test_host_bypass() {
    ChainConfig c = baseConfig();
    c.highpassHz = 200.0;
    VoiceChain vc;
    prepared(vc, c);
    vc.setHostBypass(true);
    auto l = sine(100, 0.4, 0.5), r = l, ol = l, orr = r;
    runStereo(vc, l, r, ol, orr);
    size_t start = static_cast<size_t>(0.1 * SR);
    bool identical = true;
    for (size_t i = start; i < l.size(); ++i) identical &= (ol[i] == l[i]);
    CHECK(identical, "host bypass must pass the input through");
}

static void test_parse_fixture() {
    std::ifstream f(std::string(SC_FIXTURE_DIR) + "/chain.json");
    std::stringstream ss;
    ss << f.rdbuf();
    auto c = parseChainJson(ss.str());
    CHECK(c.has_value(), "fixture should parse");
    if (!c) return;
    CHECK(c->monoDownmix, "mono");
    CHECK(c->highpassHz == 90.0 && c->highpassSlopeDbOct == 24, "hpf %.1f/%d", c->highpassHz, c->highpassSlopeDbOct);
    CHECK(c->eq.size() == 3, "eq bands %zu", c->eq.size());
    CHECK(c->eq[0].kind == EqKind::Peak && c->eq[0].gainDb == -3.5, "band 0");
    CHECK(c->eq[1].kind == EqKind::HighShelf && c->eq[2].kind == EqKind::LowShelf, "shelf kinds");
    CHECK(c->deesser && c->deesser->maxReductionDb == 6.0 && c->deesser->thresholdDb == -8.0, "deesser");

    CHECK(!parseChainJson("{not json").has_value(), "garbage → pass-through");
    CHECK(!parseChainJson("{\"mono_downmix\": true}").has_value(), "missing highpass → pass-through");
    CHECK(!parseChainJson("{\"highpass_hz\": 80, \"eq\": [{\"kind\": \"wobble\"}]}").has_value(), "unknown eq kind");
    std::string many = "{\"highpass_hz\": 80, \"deesser\": null, \"eq\": [";
    for (int i = 0; i < 7; ++i) many += std::string(i ? "," : "") + "{\"kind\":\"peak\",\"freq_hz\":1000,\"gain_db\":-1,\"q\":1}";
    many += "]}";
    auto m = parseChainJson(many);
    CHECK(m && m->eq.size() == kMaxEqBands && !m->deesser, "eq capped at 5, null de-esser");
    auto clamped = parseChainJson("{\"highpass_hz\": 5000, \"eq\": [{\"kind\":\"peak\",\"freq_hz\":1000,\"gain_db\":-40,\"q\":1}]}");
    CHECK(clamped && clamped->highpassHz == 1000.0 && clamped->eq[0].gainDb == -12.0, "values clamped");
}

static void test_watcher_follows_file() {
    namespace fs = std::filesystem;
    fs::path dir = fs::temp_directory_path() / ("setupcoach-test-" + std::to_string(std::random_device{}()));
    fs::create_directories(dir);
    fs::path file = dir / "chain.json";
    VoiceChain vc;
    vc.prepare(SR, 512);
    ChainWatcher w(vc, file);
    CHECK(w.pollOnce(), "first poll publishes");
    CHECK(!vc.hasConfig(), "no file yet");
    { std::ofstream(file) << "{\"highpass_hz\": 100}"; }
    CHECK(w.pollOnce() && vc.hasConfig(), "file appeared");
    CHECK(!w.pollOnce(), "unchanged file is not re-published");
    { std::ofstream(file) << "{broken"; }
    w.pollOnce();
    CHECK(!vc.hasConfig(), "invalid file → pass-through");
    fs::remove(file);
    w.pollOnce();
    CHECK(!vc.hasConfig(), "deleted file → pass-through");
    // Background thread smoke test.
    { std::ofstream(file) << "{\"highpass_hz\": 120}"; }
    ChainWatcher bg(vc, file, std::chrono::milliseconds(20));
    bg.start();
    std::this_thread::sleep_for(std::chrono::milliseconds(80));
    bg.stop();
    CHECK(vc.hasConfig(), "background watcher loaded the file");
    fs::remove_all(dir);
}

static void test_config_change_has_no_click() {
    ChainConfig a = baseConfig();
    a.highpassHz = 60.0;
    ChainConfig b = baseConfig();
    b.highpassHz = 150.0;
    b.eq.push_back({EqKind::Peak, 300, -6.0, 1.0});
    b.deesser = DeEsserConfig{};
    VoiceChain vc;
    prepared(vc, a);
    auto in = sine(220, 0.5, 1.0);
    std::vector<float> out(in.size());
    const size_t block = 256;
    double maxStep = 0, steadyStep = 0;
    for (size_t i = 0; i < in.size(); i += block) {
        if (i == block * 100) vc.setConfig(b);  // switch mid-stream
        if (i == block * 60) vc.setConfig(std::nullopt), vc.setConfig(a);  // replace before pickup
        int n = static_cast<int>(std::min(block, in.size() - i));
        const float* p[1] = {in.data() + i};
        vc.process(p, 1, out.data() + i, n);
    }
    for (size_t i = 1; i < out.size(); ++i) {
        double s = std::fabs(out[i] - out[i - 1]);
        if (i > block * 100) maxStep = std::max(maxStep, s);
        else if (i > block * 20) steadyStep = std::max(steadyStep, s);
    }
    // A 220 Hz sine at 0.5 moves at most ~0.0144 per sample; allow a little.
    CHECK(maxStep < steadyStep * 1.5 + 1e-3, "config switch caused a jump: %.4f vs steady %.4f", maxStep, steadyStep);
    vc.collectGarbage();
}

static void test_rate_independent_deesser_timing() {
    // Same de-esser behaviour at 44.1 kHz: still reduces sibilance.
    ChainConfig with = baseConfig();
    with.deesser = DeEsserConfig{6500.0, -8.0, 6.0};
    VoiceChain a, b;
    a.prepare(44100.0, 512);
    a.setConfig(with);
    b.prepare(44100.0, 512);
    b.setConfig(baseConfig());
    auto ess = hfNoise(1.5, 0.3, 7);
    double red = tailDb(runMono(b, ess)) - tailDb(runMono(a, ess));
    CHECK(red > 4.0, "44.1 kHz de-esser reduction %.2f dB", red);
}

static const char* kDeEsserJsonHead = "{\"highpass_hz\": 20, \"deesser\": ";

static std::optional<DeEsserConfig> parseDe(const std::string& deJson) {
    auto c = parseChainJson(kDeEsserJsonHead + deJson + "}");
    if (!c) return std::nullopt;
    return c->deesser;
}

static void test_deesser_centre_in_chain_json() {
    auto a = parseDe("{\"freq_hz\": 4500, \"threshold_db\": -9, \"max_reduction_db\": 5}");
    CHECK(a && a->freqHz == 4500.0 && a->thresholdDb == -9.0 && a->maxReductionDb == 5.0, "4.5 kHz centre read");
    auto b = parseDe("{\"freq_hz\": 9000, \"threshold_db\": -9, \"max_reduction_db\": 5}");
    CHECK(b && b->freqHz == 9000.0, "9 kHz centre read");
    // Old files without freq_hz (or with a useless one) keep the fixed centre.
    for (const char* old : {"{\"threshold_db\": -8, \"max_reduction_db\": 6}", "{}", "{\"freq_hz\": null}",
                            "{\"freq_hz\": \"high\"}"}) {
        auto d = parseDe(old);
        CHECK(d && d->freqHz == 6500.0 && d->freqHz == kDefaultDeEsserHz, "default centre for %s", old);
    }
    auto lo = parseDe("{\"freq_hz\": 100}");
    auto hi = parseDe("{\"freq_hz\": 40000}");
    CHECK(lo && lo->freqHz == 2000.0 && hi && hi->freqHz == 12000.0, "centre clamped to 2-12 kHz");
    // A de-esser block with a bad threshold type must not invalidate the chain.
    auto t = parseDe("{\"freq_hz\": 7000, \"threshold_db\": \"x\"}");
    CHECK(t && t->freqHz == 7000.0 && t->thresholdDb == -8.0, "bad threshold keeps default");

    // Old chain without freq_hz renders exactly like an explicit 6.5 kHz centre.
    auto oldCfg = parseChainJson("{\"highpass_hz\": 20, \"deesser\": {\"threshold_db\": -8, \"max_reduction_db\": 6}}");
    ChainConfig explicitCfg = baseConfig();
    explicitCfg.deesser = DeEsserConfig{6500.0, -8.0, 6.0};
    VoiceChain va, vb;
    prepared(va, oldCfg);
    prepared(vb, explicitCfg);
    auto ess = hfNoise(1.0, 0.3, 3);
    auto ya = runMono(va, ess), yb = runMono(vb, ess);
    bool same = true;
    for (size_t i = 0; i < ya.size(); ++i) same &= (ya[i] == yb[i]);
    CHECK(same, "chain.json without freq_hz must match the 6.5 kHz default exactly");
}

static void test_deesser_centre_selects_band() {
    // Threshold -3 dB: only a tone inside the side-chain band counts as an 'ess'.
    // The Q=1 band-pass is -5 dB one octave away, so the wrong centre stays quiet.
    struct Case { double tone, centre; bool expectCut; };
    for (Case cs : {Case{4500, 4500, true}, Case{4500, 9000, false}, Case{9000, 9000, true}, Case{9000, 4500, false},
                    Case{7000, 7000, true}}) {
        ChainConfig with = baseConfig();
        with.deesser = DeEsserConfig{cs.centre, -3.0, 6.0};
        VoiceChain a, b;
        prepared(a, with);
        prepared(b, baseConfig());
        auto tone = sine(cs.tone, 0.3, 1.5);
        double cut = tailDb(runMono(b, tone)) - tailDb(runMono(a, tone));
        if (cs.expectCut) CHECK(cut > 3.0, "tone %.0f Hz, centre %.0f Hz: only %.2f dB cut", cs.tone, cs.centre, cut);
        else CHECK(cut < 0.1, "tone %.0f Hz, centre %.0f Hz: cut %.2f dB, expected none", cs.tone, cs.centre, cut);
    }
}

// --- Plugin vs. core's offline renderer ------------------------------------
// Deterministic synthetic sibilant voice, generated identically in Rust
// (core/src/audio/process.rs rendering a chain with only HPF 20 Hz + de-esser
// active; expander off, compressor ratio 1, limiter 0 dB). Voiced vowel
// (120 Hz harmonics, 1/h) for 0.3 s, then 0.2 s of 4.5-9 kHz noise, repeating.
static std::vector<float> sibilantVoice(double secs) {
    uint64_t state = 12345;
    auto next = [&state]() {
        state = state * 6364136223846793005ULL + 1442695040888963407ULL;
        return static_cast<double>(state >> 11) / 9007199254740992.0 * 2.0 - 1.0;
    };
    Biquad h1 = Biquad::highpass(SR, 4500, 0.707), h2 = Biquad::highpass(SR, 4500, 0.707);
    Biquad lp = Biquad::lowpass(SR, 9000, 0.707);
    std::vector<float> x(static_cast<size_t>(secs * SR));
    for (size_t i = 0; i < x.size(); ++i) {
        double t = static_cast<double>(i) / SR;
        double noise = lp.process(h2.process(h1.process(next())));
        double v = 0.0;
        if (std::fmod(t, 0.5) < 0.3) {
            for (int h = 1; h <= 30; ++h) v += std::sin(2.0 * kPi * 120.0 * h * t) * 0.1 / h;
        } else {
            v = 0.35 * noise;
        }
        x[i] = static_cast<float>(v);
    }
    return x;
}

// {loudness over the whole signal, level of the 5-9 kHz band during the 's' bursts}, dB, after 0.5 s.
static std::pair<double, double> voiceMetrics(const std::vector<float>& x) {
    size_t skip = static_cast<size_t>(0.5 * SR);
    Biquad b1 = Biquad::highpass(SR, 5000, 0.707), b2 = Biquad::highpass(SR, 5000, 0.707), b3 = Biquad::lowpass(SR, 9000, 0.707);
    double all = 0, ess = 0;
    size_t nEss = 0;
    for (size_t i = 0; i < x.size(); ++i) {
        double band = b3.process(b2.process(b1.process(x[i])));
        if (i < skip) continue;
        all += double(x[i]) * x[i];
        if (std::fmod(static_cast<double>(i) / SR, 0.5) >= 0.3) {
            ess += band * band;
            ++nEss;
        }
    }
    return {10 * std::log10(all / (x.size() - skip)), 10 * std::log10(ess / std::max<size_t>(nEss, 1))};
}

static void test_plugin_matches_renderer() {
    auto x = sibilantVoice(4.0);
    auto dry = voiceMetrics(x);
    // Values from coach_core::audio::process::render on the same signal.
    CHECK(std::fabs(dry.first - -21.347) < 0.02 && std::fabs(dry.second - -26.451) < 0.02,
          "synthetic voice differs from the Rust one: %.3f / %.3f", dry.first, dry.second);
    struct Golden { double centre, loudDb, essDb; };
    for (Golden g : {Golden{4500, -22.716, -33.350}, Golden{6500, -22.696, -33.433}, Golden{9000, -22.525, -31.884}}) {
        ChainConfig c = baseConfig();
        c.deesser = DeEsserConfig{g.centre, -8.0, 6.0};
        VoiceChain vc;
        prepared(vc, c);
        auto m = voiceMetrics(runMono(vc, x));
        std::printf("renderer-sync centre %.0f: loudness %.3f (dry %.3f), s band %.3f (dry %.3f) [renderer %.3f / %.3f]\n",
                    g.centre, m.first, dry.first, m.second, dry.second, g.loudDb, g.essDb);
        CHECK(std::fabs(m.first - g.loudDb) < 0.05, "centre %.0f: loudness %.3f vs renderer %.3f", g.centre, m.first, g.loudDb);
        CHECK(std::fabs(m.second - g.essDb) < 0.05, "centre %.0f: s band %.3f vs renderer %.3f", g.centre, m.second, g.essDb);
    }
}

int main() {
    test_highpass();
    test_eq_bands();
    test_left_only_reaches_both_ears();
    test_deesser();
    test_passthrough_when_file_missing();
    test_host_bypass();
    test_parse_fixture();
    test_watcher_follows_file();
    test_config_change_has_no_click();
    test_rate_independent_deesser_timing();
    test_deesser_centre_in_chain_json();
    test_deesser_centre_selects_band();
    test_plugin_matches_renderer();
    std::printf("%d checks, %d failures\n", g_checks, g_failures);
    return g_failures == 0 ? 0 : 1;
}
