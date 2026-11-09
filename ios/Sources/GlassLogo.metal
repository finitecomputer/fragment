#include <metal_stdlib>
#include <SwiftUI/SwiftUI_Metal.h>
using namespace metal;

// One pass over the layer behind the logo, in place of a 3D renderer: every
// pixel outside the logo returns untouched, and one inside costs six height
// reads and fifteen layer reads. `height` is the logo's bars as domes.

// Frost: a small disc of taps, turned per pixel so five read as a soft grain.
constant float2 frostTaps[5] = { float2(0.0, 0.0), float2(0.9, 0.3), float2(-0.3, 0.9), float2(-0.9, -0.3), float2(0.3, -0.9) };

static float heightAt(texture2d<half> height, float2 uv) {
    constexpr sampler s(address::clamp_to_zero, filter::linear);
    return float(height.sample(s, uv).r);
}

[[ stitchable ]] half4 glassLogo(float2 position, SwiftUI::Layer layer, float2 bounds, float2 center, float size, float2 tilt, texture2d<half> height) {
    float2 p = (position - center) / size;
    if (any(abs(p) > 0.75)) { return layer.sample(position); }

    // Turning the logo: foreshortened along the tilt, nearer on its raised side.
    float2 turned = p / max(float2(cos(tilt.x), cos(tilt.y)), 0.5);
    turned /= 1.0 + dot(p, sin(tilt)) * 0.35;
    float2 uv = turned + 0.5;

    float h = heightAt(height, uv);
    // The side wall: the back of the logo shows on its nearer side.
    float wall = heightAt(height, uv - sin(tilt) * 0.03);
    float body = max(h, wall * 0.85);
    if (body < 0.004) { return layer.sample(position); }

    const float e = 1.0 / 448.0;
    float2 slope = float2(heightAt(height, uv + float2(e, 0)) - heightAt(height, uv - float2(e, 0)),
                          heightAt(height, uv + float2(0, e)) - heightAt(height, uv - float2(0, e)));
    float3 normal = normalize(float3(-slope * 5.0, 1.0));

    // Refraction through a thick lens, each channel bent by its own index.
    float2 bend = (normal.xy * 0.22 - sin(tilt) * 0.10) * size;
    float turn = fract(sin(dot(floor(position * 3.0), float2(12.9898, 78.233))) * 43758.5453) * 6.2832;
    float2x2 spin = float2x2(cos(turn), sin(turn), -sin(turn), cos(turn));
    half3 frosted = half3(0.0h);
    for (int i = 0; i < 5; i++) {
        float2 tap = position + spin * frostTaps[i] * size * 0.035;
        frosted += half3(layer.sample(clamp(tap + bend * 0.92, 0.0, bounds)).r,
                         layer.sample(clamp(tap + bend, 0.0, bounds)).g,
                         layer.sample(clamp(tap + bend * 1.08, 0.0, bounds)).b);
    }
    half4 color = half4(frosted / 5.0h, 1.0h);

    // Fresnel at the rim, and two lights held still as the logo turns.
    float3 view = float3(0, 0, 1);
    float rim = pow(1.0 - saturate(dot(normal, view)), 2.5);
    float3 key = normalize(float3(-0.45 + tilt.x * 1.6, -0.7 + tilt.y * 1.6, 0.75));
    float3 fill = normalize(float3(0.6 + tilt.x * 1.6, 0.55 + tilt.y * 1.6, 0.7));
    float glint = pow(saturate(dot(reflect(-key, normal), view)), 48.0);
    float sheen = pow(saturate(dot(reflect(-fill, normal), view)), 14.0);

    half3 glass = mix(color.rgb, half3(0.92h, 0.95h, 1.0h), 0.14h) + half3(0.10h, 0.12h, 0.15h) * half(0.3 + 0.3 * h);
    glass = mix(glass, half3(0.02h, 0.05h, 0.12h), half(rim * 0.55));
    glass += half(glint * 1.1) + half3(0.55h, 0.75h, 1.0h) * half(sheen * 0.22);

    half coverage = half(smoothstep(0.004, 0.06, body));
    return mix(layer.sample(position), half4(glass, 1.0h), coverage);
}
