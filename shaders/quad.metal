// language: metal2.0
#include <metal_stdlib>
#include <simd/simd.h>

using metal::uint;

constexpr constant float c_scale = 1.2000000476837158;
struct VertexOutput {
    metal::float2 uv;
    metal::float4 position;
};

struct vert_mainInput {
    metal::float2 pos [[attribute(0)]];
    metal::float2 uv [[attribute(1)]];
};
struct vert_mainOutput {
    metal::float2 uv [[user(loc0), center_perspective]];
    metal::float4 position [[position]];
};
vertex vert_mainOutput vert_main(
  vert_mainInput varyings [[stage_in]]
) {
    const auto pos = varyings.pos;
    const auto uv = varyings.uv;
    const auto _tmp = VertexOutput {uv, metal::float4(c_scale * pos, 0.0, 1.0)};
    return vert_mainOutput { _tmp.uv, _tmp.position };
}


struct frag_mainInput {
    metal::float2 uv_1 [[user(loc0), center_perspective]];
};
struct frag_mainOutput {
    metal::float4 member_1 [[color(0)]];
};
fragment frag_mainOutput frag_main(
  frag_mainInput varyings_1 [[stage_in]]
, metal::texture2d<float, metal::access::sample> u_texture [[user(fake0)]]
, metal::sampler u_sampler [[user(fake0)]]
) {
    const auto uv_1 = varyings_1.uv_1;
    metal::float4 color = u_texture.sample(u_sampler, uv_1);
    if (color.w == 0.0) {
        metal::discard_fragment();
    }
    metal::float4 premultiplied = color.w * color;
    return frag_mainOutput { premultiplied };
}


struct fs_extraOutput {
    metal::float4 member_2 [[color(0)]];
};
fragment fs_extraOutput fs_extra(
) {
    return fs_extraOutput { metal::float4(0.0, 0.5, 0.0, 0.5) };
}
