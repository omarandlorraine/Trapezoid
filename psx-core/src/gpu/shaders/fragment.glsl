#version 450

layout(location = 0) in vec3 v_color;
layout(location = 1) in vec2 v_tex_coord;

layout(location = 0) out vec4 f_color;

layout(push_constant) uniform PushConstantData {
    ivec2 offset;
    uvec2 drawing_top_left;
    uvec2 drawing_size;

    bool semi_transparent;
    uint semi_transparency_mode;

    bool dither_enabled;

    bool is_textured;
    uint texture_width;
    bool is_texture_blended;
    uint tex_page_color_mode;
    bvec2 texture_flip;
} pc;

layout(set = 0, binding = 0) buffer TextureData {
    uint data[];
} tex;
layout(set = 0, binding = 1) buffer ClutData {
    uint data[];
} clut;
layout(set = 0, binding = 2) uniform sampler2D back_tex;


const vec2 screen_dim = vec2(1024, 512);

const float transparency_factors[4][2] = {
    // back factor, front factor
    {0.5, 0.5},
    {1.0, 1.0},
    {1.0, -1.0},
    {1.0, 0.25},
};

const float dither_table[16] = {
    -4.0/255.0,  +0.0/255.0,  -3.0/255.0,  +1.0/255.0,   //\dither offsets for first two scanlines
    +2.0/255.0,  -2.0/255.0,  +3.0/255.0,  -1.0/255.0,   ///
    -3.0/255.0,  +1.0/255.0,  -4.0/255.0,  +0.0/255.0,   //\dither offsets for next two scanlines
    +3.0/255.0,  -1.0/255.0,  +2.0/255.0,  -2.0/255.0    ///(same as above, but shifted two pixels horizontally)
};


vec3 get_color_from_u16(uint color_texel) {
    uint r = color_texel & 0x1Fu;
    uint g = (color_texel >> 5) & 0x1Fu;
    uint b = (color_texel >> 10) & 0x1Fu;

    return vec3(r, g, b) / 31.0;
}

bool alpha_from_u16(uint color_texel) {
    return (color_texel & 0x8000u) != 0;
}

vec3 get_color_with_semi_transparency(vec3 color, bool semi_transparency_param) {
    if (!semi_transparency_param) {
        return color;
    }

    vec3 back_color = vec3(texture(back_tex, gl_FragCoord.xy / screen_dim));

    float factors[] = transparency_factors[pc.semi_transparency_mode];

    return (factors[0] * back_color) + (factors[1] * color);
}

void main() {
    vec3 t_color;
    if (pc.is_textured) {
        uvec2 tex_coord = uvec2(round(v_tex_coord));

        // how many pixels in 16 bit
        // 0 => 4
        // 1 => 2
        // 2 => 1
        uint divider = 1 << (2 - pc.tex_page_color_mode);
        uint texture_width = 1 << (6 + pc.tex_page_color_mode);

        uint x = tex_coord.x / divider;
        uint y = tex_coord.y;

        // texture flips
        if (pc.texture_flip.x) {
            x = (255u / divider) - x;
        }
        if (pc.texture_flip.y) {
            y = 255u - y;
        }

        // since this is u32 datatype, we need to manually extract
        // the u16 data
        uint color_value = tex.data[((y * texture_width) + x ) / 2];
        if (x % 2 == 0) {
            color_value = color_value & 0xFFFF;
        } else {
            color_value = color_value >> 16;
        }

        // if we need clut, then compute it
        if (pc.tex_page_color_mode == 0u || pc.tex_page_color_mode == 1u) {
            uint mask = 0xFFFFu >> (16u - (16u / divider));
            uint clut_index_shift = (tex_coord.x % divider) * (16u / divider);
            uint clut_index = (color_value >> clut_index_shift) & mask;

            x = int(clut_index);
            // since this is u32 datatype, we need to manually extract
            // the u16 data
            color_value = clut.data[x / 2];
            if (x % 2 == 0) {
                color_value = color_value & 0xFFFF;
            } else {
                color_value = color_value >> 16;
            }
        }

        // if its all 0, then its transparent
        if (color_value == 0u){
            discard;
        }

        vec3 color = get_color_from_u16(color_value);
        bool alpha = alpha_from_u16(color_value);

        if (pc.is_texture_blended) {
            color *=  v_color * 2;
        }
        t_color = get_color_with_semi_transparency(color, alpha);
    } else {
        if (pc.dither_enabled) {
            uint x = uint(gl_FragCoord.x) % 4;
            uint y = uint(gl_FragCoord.y) % 4;

            float change = dither_table[y * 4 + x];
            t_color = v_color + change;
        } else {
            t_color = v_color;
        }

        t_color = get_color_with_semi_transparency(t_color, pc.semi_transparent);
    }
    f_color = vec4(t_color.b, t_color.g, t_color.r, 0.0);
}
