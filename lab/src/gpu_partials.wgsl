// lab.gpu_partials (lab/src/gpu_partials.rs): y[t] = Σ_i a_i·sin(2π·(φ_i + inc_i·t)),
// one invocation per frame t of a 256-frame block. Each partial is
// (φ, inc_hi, inc_lo, a); inc_hi has 15 fractional bits, so inc_hi·t is exact
// in f32 for t < 256. The sum runs in a fixed order, 32 groups of 32 partials.

const PARTIALS: u32 = 1024u;
const GROUP: u32 = 32u;
const FRAMES: u32 = 256u;
const TAU: f32 = 6.28318530717958647692;

@group(0) @binding(0) var<storage, read> input: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;

@compute @workgroup_size(64)
fn partials(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = id.x;
    if (t >= FRAMES) {
        return;
    }
    let tf = f32(t);
    var total = 0.0;
    for (var g = 0u; g < PARTIALS; g += GROUP) {
        var sum = 0.0;
        for (var i = g; i < g + GROUP; i++) {
            let p = input[i];
            let cycles = fract(p.x + fract(p.y * tf) + p.z * tf);
            // In [−½, ½): the argument stays in [−π, π].
            let centred = cycles - floor(cycles + 0.5);
            sum += p.w * sin(TAU * centred);
        }
        total += sum;
    }
    output[t] = total;
}
