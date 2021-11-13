use vulkano::buffer::{BufferUsage, CpuAccessibleBuffer};
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, PrimaryAutoCommandBuffer, PrimaryCommandBuffer,
    SubpassContents,
};
use vulkano::device::{Device, Queue};
use vulkano::format::{ClearValue, Format};
use vulkano::image::view::ImageView;
use vulkano::image::{ImageAccess, ImageDimensions, StorageImage};
use vulkano::pipeline::viewport::Viewport;
use vulkano::pipeline::GraphicsPipeline;
use vulkano::render_pass::{Framebuffer, Subpass};
use vulkano::sampler::Filter;
use vulkano::sync::{self, GpuFuture};

use super::GpuStat;

use std::ops::Range;
use std::sync::Arc;

mod vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        path: "src/gpu/shaders/vertex.glsl"
    }
}

mod fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "src/gpu/shaders/fragment.glsl"
    }
}

/// helper to convert opengl colors into u16
#[inline]
fn gl_pixel_to_u16(pixel: &(u8, u8, u8, u8)) -> u16 {
    ((pixel.3 & 1) as u16) << 15
        | ((pixel.2 >> 3) as u16) << 10
        | ((pixel.1 >> 3) as u16) << 5
        | (pixel.0 >> 3) as u16
}

/// helper in getting the correct value for bottom for gl drawing/coordinate stuff
#[inline]
fn to_gl_bottom(top: u32, height: u32) -> u32 {
    512 - height - top
}

#[inline]
pub fn vertex_position_from_u32(position: u32) -> [f32; 2] {
    let x = position & 0x7ff;
    let sign_extend = 0xfffff800 * ((x >> 10) & 1);
    let x = (x | sign_extend) as i32;
    let y = (position >> 16) & 0x7ff;
    let sign_extend = 0xfffff800 * ((y >> 10) & 1);
    let y = (y | sign_extend) as i32;
    [x as f32, y as f32]
}

#[derive(Copy, Clone, Debug, Default)]
pub struct DrawingVertex {
    position: [f32; 2],
    color: [f32; 3],
    tex_coord: [u32; 2],
}

impl DrawingVertex {
    #[inline]
    pub fn position(&self) -> [f32; 2] {
        self.position
    }

    #[inline]
    pub fn set_position(&mut self, position: [f32; 2]) {
        self.position = position;
    }

    #[inline]
    pub fn tex_coord(&mut self) -> [u32; 2] {
        self.tex_coord
    }

    #[inline]
    pub fn set_tex_coord(&mut self, tex_coord: [u32; 2]) {
        self.tex_coord = tex_coord;
    }

    #[inline]
    pub fn new_with_color(color: u32) -> Self {
        let mut s = Self::default();
        s.color_from_u32(color);
        s
    }

    #[inline]
    pub fn position_from_u32(&mut self, position: u32) {
        self.position = vertex_position_from_u32(position);
    }

    #[inline]
    pub fn color_from_u32(&mut self, color: u32) {
        let r = (color & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = ((color >> 16) & 0xFF) as u8;

        self.color = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
    }

    #[inline]
    pub fn tex_coord_from_u32(&mut self, tex_coord: u32) {
        self.tex_coord = [(tex_coord & 0xFF), ((tex_coord >> 8) & 0xFF)];
    }
}

vulkano::impl_vertex!(DrawingVertex, position, color, tex_coord);

#[derive(Copy, Clone, Debug, Default)]
pub struct DrawingTextureParams {
    clut_base: [u32; 2],
    tex_page_base: [u32; 2],
    semi_transparency_mode: u8,
    tex_page_color_mode: u8,
    texture_disable: bool,
    texture_flip: (bool, bool),
}

impl DrawingTextureParams {
    /// Process tex page params, from the lower 16 bits, this is only used
    /// for when drawing rectangle, as the tex_page is take fron the gpu_stat
    /// and not from a parameter
    #[inline]
    pub fn tex_page_from_gpustat(&mut self, param: u32) {
        let x = param & 0xF;
        let y = (param >> 4) & 1;

        self.tex_page_base = [x * 64, y * 256];
        self.semi_transparency_mode = ((param >> 5) & 3) as u8;
        self.tex_page_color_mode = ((param >> 7) & 3) as u8;
        self.texture_disable = (param >> 11) & 1 == 1;
    }

    /// Process tex page params, from the higher 16 bits, which is found
    /// in tex page parameter in drawing stuff
    #[inline]
    pub fn tex_page_from_u32(&mut self, param: u32) {
        let param = param >> 16;
        self.tex_page_from_gpustat(param);
    }

    #[inline]
    pub fn clut_from_u32(&mut self, param: u32) {
        let param = param >> 16;
        let x = param & 0x3F;
        let y = (param >> 6) & 0x1FF;
        self.clut_base = [x * 16, y];
    }

    #[inline]
    pub fn set_texture_flip(&mut self, flip: (bool, bool)) {
        self.texture_flip = flip;
    }
}

pub struct Vram {
    data: Arc<CpuAccessibleBuffer<[u16]>>,
}

impl Vram {
    #[inline]
    fn new(device: Arc<Device>) -> Self {
        let data = CpuAccessibleBuffer::from_iter(
            device,
            BufferUsage::all(),
            false,
            (0..1024 * 512 * 2).map(|_| 0),
        )
        .unwrap();

        Self { data }
    }

    #[inline]
    fn write_block(&mut self, block_range: &(Range<u32>, Range<u32>), block: &[u16]) {
        todo!()
        //let (x_range, y_range) = block_range;
        //let whole_size = x_range.len() * y_range.len();
        //assert_eq!(block.len(), whole_size);

        //let mut mapping = self.data.map_write();
        //let mut block_iter = block.iter();

        //for y in y_range.clone() {
        //    let mut current_pixel_pos = (y * 1024 + x_range.start) as usize;
        //    for _ in 0..x_range.len() {
        //        mapping.set(current_pixel_pos, *block_iter.next().unwrap());
        //        current_pixel_pos += 1;
        //    }
        //}

        //assert!(block_iter.next().is_none());
    }

    #[inline]
    fn read_block(&mut self, block_range: &(Range<u32>, Range<u32>), reverse: bool) -> Vec<u16> {
        todo!()
        //let (x_range, y_range) = block_range;

        //let row_size = x_range.len();
        //let whole_size = row_size * y_range.len();
        //let mut block = Vec::with_capacity(whole_size);

        //let mapping = self.data.map_read();

        //let y_range_iter: Box<dyn Iterator<Item = _>> = if reverse {
        //    Box::new(y_range.clone().rev())
        //} else {
        //    Box::new(y_range.clone())
        //};

        //for y in y_range_iter {
        //    let row_start_addr = y * 1024 + x_range.start;
        //    block.extend_from_slice(
        //        &mapping[(row_start_addr as usize)..(row_start_addr as usize + row_size)],
        //    );
        //}

        //assert_eq!(block.len(), whole_size);

        //block
    }
}

pub struct GpuContext {
    pub(super) gpu_stat: GpuStat,
    pub(super) allow_texture_disable: bool,
    pub(super) textured_rect_flip: (bool, bool),
    pub(super) gpu_read: Option<u32>,
    pub(super) _vram: Vram,

    pub(super) drawing_area_top_left: (u32, u32),
    pub(super) drawing_area_bottom_right: (u32, u32),
    pub(super) drawing_offset: (i32, i32),
    pub(super) texture_window_mask: (u32, u32),
    pub(super) texture_window_offset: (u32, u32),

    pub(super) vram_display_area_start: (u32, u32),
    pub(super) display_horizontal_range: (u32, u32),
    pub(super) display_vertical_range: (u32, u32),

    // These are only used for handleing GP1(0x10) command, so instead of creating
    // the values again from the individual parts, we just cache it
    pub(super) cached_gp0_e2: u32,
    pub(super) cached_gp0_e3: u32,
    pub(super) cached_gp0_e4: u32,
    pub(super) cached_gp0_e5: u32,

    device: Arc<Device>,
    queue: Arc<Queue>,
    render_image: Arc<StorageImage>,
    // TODO: fix this type
    render_image_framebuffer: Arc<Framebuffer<((), Arc<ImageView<Arc<StorageImage>>>)>>,
    pipeline: Arc<GraphicsPipeline>,
    // TODO: this buffer gives Gpu lock issues, so either we create
    //  buffer every time, we draw, or we create multiple buffers and loop through them
    _vertex_buffer: Arc<CpuAccessibleBuffer<[DrawingVertex]>>,

    gpu_future: Option<Box<dyn GpuFuture>>,
    // /// Stores the VRAM content to be used for rendering in the shaders
    // texture_buffer: Texture2d,
    // Ranges in the VRAM which are not resident in the VRAM at the moment but in the
    // [drawing_texture], so if any byte in this range is read/written to, then
    // we need to retrieve it from the texture and not the VRAM array
    // ranges_in_rendering: Vec<(Range<u32>, Range<u32>)>
}

impl GpuContext {
    pub fn new(device: Arc<Device>, queue: Arc<Queue>) -> Self {
        let render_image = StorageImage::new(
            device.clone(),
            ImageDimensions::Dim2d {
                width: 1024,
                height: 512,
                array_layers: 1,
            },
            Format::R5G5B5A1_UNORM_PACK16,
            [queue.family()],
        )
        .unwrap();

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                device.clone(),
                queue.family(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        // TODO: add blit commands
        builder
            .clear_color_image(
                render_image.clone(),
                ClearValue::Float([0.0, 0.0, 0.0, 0.0]),
            )
            .unwrap();
        // add command to clear the render image, and keep the future
        // for stacking later
        let command_buffer = builder.build().unwrap();
        let gpu_future = Some(
            command_buffer
                .execute(queue.clone())
                .unwrap()
                .then_signal_fence_and_flush()
                .unwrap()
                .boxed(),
        );

        let vs = vs::Shader::load(device.clone()).unwrap();
        let fs = fs::Shader::load(device.clone()).unwrap();

        let render_pass = Arc::new(
            vulkano::single_pass_renderpass!(
                device.clone(),
                attachments: {
                    color: {
                        load: Load,
                        store: Store,
                        format: Format::R5G5B5A1_UNORM_PACK16,
                        samples: 1,
                    }
                },
                pass: {
                    color: [color],
                    depth_stencil: {}
                }
            )
            .unwrap(),
        );

        let pipeline = Arc::new(
            GraphicsPipeline::start()
                .vertex_input_single_buffer::<DrawingVertex>()
                .vertex_shader(vs.main_entry_point(), ())
                .triangle_strip()
                .viewports_dynamic_scissors_irrelevant(1)
                .fragment_shader(fs.main_entry_point(), ())
                .render_pass(Subpass::from(render_pass.clone(), 0).unwrap())
                .build(device.clone())
                .unwrap(),
        );

        let render_image_framebuffer = Arc::new(
            Framebuffer::start(render_pass.clone())
                .add(ImageView::new(render_image.clone()).unwrap())
                .unwrap()
                .build()
                .unwrap(),
        );

        let vertex_buffer = CpuAccessibleBuffer::from_iter(
            device.clone(),
            BufferUsage::all(),
            false,
            [DrawingVertex::default(); 4].iter().cloned(),
        )
        .unwrap();

        //let program = program!(&gl_context,
        //    140 => {
        //        vertex: "
        //            #version 140
        //            in vec2 position;
        //            in vec3 color;
        //            in uvec2 tex_coord;

        //            out vec3 v_color;
        //            out vec2 v_tex_coord;

        //            uniform ivec2 offset;
        //            uniform uvec2 drawing_top_left;
        //            uniform uvec2 drawing_size;

        //            void main() {
        //                float posx = (position.x + offset.x - drawing_top_left.x) / drawing_size.x * 2 - 1;
        //                float posy = (position.y + offset.y - drawing_top_left.x) / drawing_size.y * (-2) + 1;

        //                gl_Position = vec4(posx, posy, 0.0, 1.0);
        //                v_color = color;
        //                v_tex_coord = vec2(tex_coord);
        //            }
        //        ",
        //        fragment: "
        //            #version 140

        //            in vec3 v_color;
        //            in vec2 v_tex_coord;

        //            out vec4 out_color;

        //            uniform bool texture_flip_x;
        //            uniform bool texture_flip_y;
        //            uniform bool is_textured;
        //            uniform bool is_texture_blended;
        //            uniform bool semi_transparent;
        //            uniform uint semi_transparency_mode;
        //            uniform sampler2D tex;
        //            uniform uvec2 tex_page_base;
        //            uniform uint tex_page_color_mode;
        //            uniform uvec2 clut_base;

        //            vec4 get_color_from_u16(uint color_texel) {
        //                uint r = color_texel & 0x1Fu;
        //                uint g = (color_texel >> 5) & 0x1Fu;
        //                uint b = (color_texel >> 10) & 0x1Fu;
        //                uint a = (color_texel >> 15) & 1u;

        //                return vec4(float(r) / 31.0, float(g) / 31.0, float(b) / 31.0, float(a));
        //            }

        //            vec4 get_color_with_semi_transparency(vec3 color, float semi_transparency_param) {
        //                float alpha;
        //                if (semi_transparency_mode == 0u) {
        //                    if (semi_transparency_param == 1.0) {
        //                        alpha = 0.5;
        //                    } else {
        //                        alpha = 1.0;
        //                    }
        //                } else if (semi_transparency_mode == 1u) {
        //                    alpha = semi_transparency_param;
        //                } else if (semi_transparency_mode == 2u) {
        //                    alpha = semi_transparency_param;
        //                } else {
        //                    // FIXME: inaccurate mode 3 semi transparency
        //                    //
        //                    // these numbers with the equation:
        //                    // (source * source_alpha + dest * (1 - source_alpha)
        //                    // Will result in the following cases:
        //                    // if semi=1:
        //                    //      s * 0.25 + d * 0.75
        //                    // if semi=0:
        //                    //      s * 1.0 + d * 0.0
        //                    //
        //                    // but we need
        //                    // if semi=1:
        //                    //      s * 0.25 + d * 1.00
        //                    // if semi=0:
        //                    //      s * 1.0 + d * 0.0
        //                    //
        //                    // Thus, this is not accurate, but temporary will keep
        //                    // it like this until we find a new solution
        //                    if (semi_transparency_param == 1.0) {
        //                        alpha = 0.25;
        //                    } else {
        //                        alpha = 1.0;
        //                    }
        //                }

        //                return vec4(color, alpha);
        //            }

        //            void main() {
        //                // retrieve the interpolated value of `tex_coord`
        //                uvec2 tex_coord = uvec2(round(v_tex_coord));

        //                if (is_textured) {
        //                    // how many pixels in 16 bit
        //                    uint divider;
        //                    if (tex_page_color_mode == 0u) {
        //                        divider = 4u;
        //                    } else if (tex_page_color_mode == 1u) {
        //                        divider = 2u;
        //                    } else {
        //                        divider = 1u;
        //                    };

        //                    // offsetted position
        //                    // FIXME: fix weird inconsistent types here (uint and int)
        //                    int x;
        //                    int y;
        //                    if (texture_flip_x) {
        //                        x = int(tex_page_base.x + (256u / divider)) - int(tex_coord.x / divider);
        //                    } else {
        //                        x = int(tex_page_base.x) + int(tex_coord.x / divider);
        //                    }
        //                    if (texture_flip_y) {
        //                        y = int(tex_page_base.y + 256u) - int(tex_coord.y);
        //                    } else {
        //                        y = int(tex_page_base.y) + int(tex_coord.y);
        //                    }

        //                    uint color_value = uint(texelFetch(tex, ivec2(x, y), 0).r * 0xFFFF);

        //                    // if we need clut, then compute it
        //                    if (tex_page_color_mode == 0u || tex_page_color_mode == 1u) {
        //                        uint mask = 0xFFFFu >> (16u - (16u / divider));
        //                        uint clut_index_shift = (tex_coord.x % divider) * (16u / divider);
        //                        if (texture_flip_x) {
        //                            clut_index_shift = 12u - clut_index_shift;
        //                        }
        //                        uint clut_index = (color_value >> clut_index_shift) & mask;

        //                        x = int(clut_base.x + clut_index);
        //                        y = int(clut_base.y);
        //                        color_value = uint(texelFetch(tex, ivec2(x, y), 0).r * 0xFFFF);
        //                    }

        //                    // if its all 0, then its transparent
        //                    if (color_value == 0u){
        //                        discard;
        //                    }

        //                    vec4 color_with_alpha = get_color_from_u16(color_value);
        //                    vec3 color = vec3(color_with_alpha);

        //                    if (is_texture_blended) {
        //                        color *=  v_color * 2;
        //                    }
        //                    out_color = get_color_with_semi_transparency(color, color_with_alpha.a);
        //                } else {
        //                    out_color = get_color_with_semi_transparency(v_color, float(semi_transparent));
        //                }
        //            }
        //        "
        //    },
        //)
        //.unwrap();

        Self {
            gpu_stat: Default::default(),
            allow_texture_disable: false,
            textured_rect_flip: (false, false),
            gpu_read: Default::default(),
            _vram: Vram::new(device.clone()),

            drawing_area_top_left: (0, 0),
            drawing_area_bottom_right: (0, 0),
            drawing_offset: (0, 0),
            texture_window_mask: (0, 0),
            texture_window_offset: (0, 0),

            cached_gp0_e2: 0,
            cached_gp0_e3: 0,
            cached_gp0_e4: 0,
            cached_gp0_e5: 0,

            vram_display_area_start: (0, 0),
            display_horizontal_range: (0, 0),
            display_vertical_range: (0, 0),
            device,
            queue,
            render_image,
            render_image_framebuffer,

            pipeline,

            _vertex_buffer: vertex_buffer,
            gpu_future,
        }
    }
}

impl GpuContext {
    /// Drawing commands that use textures will update gpustat
    fn update_gpu_stat_from_texture_params(&mut self, texture_params: &DrawingTextureParams) {
        let x = (texture_params.tex_page_base[0] / 64) & 0xF;
        let y = (texture_params.tex_page_base[1] / 256) & 1;
        self.gpu_stat.bits &= !0x81FF;
        self.gpu_stat.bits |= x;
        self.gpu_stat.bits |= y << 4;
        self.gpu_stat.bits |= (texture_params.semi_transparency_mode as u32) << 5;
        self.gpu_stat.bits |= (texture_params.tex_page_color_mode as u32) << 7;
        self.gpu_stat.bits |= (texture_params.texture_disable as u32) << 15;
    }

    fn move_from_rendering_to_vram(&mut self, range: &(Range<u32>, Range<u32>)) {
        //let width = range.0.end - range.0.start;
        //let height = range.1.end - range.1.start;
        //let tex =
        //    Texture2d::empty_with_mipmaps(&self.gl_context, MipmapsOption::NoMipmap, width, height)
        //        .unwrap();
        //self.drawing_texture.as_surface().blit_color(
        //    &Rect {
        //        left: range.0.start,
        //        bottom: to_gl_bottom(range.1.start, height),
        //        width,
        //        height,
        //    },
        //    &tex.as_surface(),
        //    &BlitTarget {
        //        left: 0,
        //        bottom: 0,
        //        width: width as i32,
        //        height: height as i32,
        //    },
        //    MagnifySamplerFilter::Nearest,
        //);

        //let pixels: Vec<_> = tex
        //    .read::<Vec<_>>()
        //    .iter()
        //    .rev()
        //    .flatten()
        //    .map(gl_pixel_to_u16)
        //    .collect();
        //self.vram.write_block(range, &pixels);
        //self.update_texture_buffer();
    }

    fn move_from_vram_to_rendering(&mut self, range: &(Range<u32>, Range<u32>)) {
        todo!()
        //let (x_range, y_range) = range;
        //let width = x_range.len() as u32;
        //let height = y_range.len() as u32;

        //let block = self.vram.read_block(range, true);

        //self.drawing_texture.write(
        //    Rect {
        //        left: x_range.start,
        //        bottom: to_gl_bottom(y_range.start, height),
        //        width,
        //        height,
        //    },
        //    RawImage2d {
        //        data: Cow::Borrowed(block.as_ref()),
        //        width,
        //        height,
        //        format: ClientFormat::U1U5U5U5Reversed,
        //    },
        //);
    }

    /// check if a whole block in rendering
    fn is_block_in_rendering(&self, block_range: &(Range<u32>, Range<u32>)) -> bool {
        todo!()
        //let positions = [
        //    (block_range.0.start, block_range.1.start),
        //    (block_range.0.end - 1, block_range.1.end - 1),
        //];

        //for range in &self.ranges_in_rendering {
        //    let contain_start =
        //        range.0.contains(&positions[0].0) && range.1.contains(&positions[0].1);
        //    let contain_end =
        //        range.0.contains(&positions[1].0) && range.1.contains(&positions[1].1);

        //    // we use or and then assert, to make sure both ends are in the rendering.
        //    //  if only one of them are in the rendering, then this is a problem.
        //    //
        //    //  TODO: fix block half present in rendering.
        //    if contain_start || contain_end {
        //        assert!(contain_start && contain_end);

        //        return true;
        //    }
        //}

        //false
    }

    fn add_to_rendering_range(&mut self, new_range: (Range<u32>, Range<u32>)) {
        todo!()
        //fn range_overlap(r1: &(Range<u32>, Range<u32>), r2: &(Range<u32>, Range<u32>)) -> bool {
        //    // they are left/right to each other
        //    if r1.0.start >= r2.0.end || r2.0.start >= r1.0.end {
        //        return false;
        //    }

        //    // they are on top of one another
        //    if r1.1.start >= r2.1.end || r2.1.start >= r1.1.end {
        //        return false;
        //    }

        //    true
        //}

        //if !self.ranges_in_rendering.contains(&new_range) {
        //    let mut overlapped_ranges = Vec::new();
        //    self.ranges_in_rendering.retain(|range| {
        //        if range_overlap(range, &new_range) {
        //            overlapped_ranges.push(range.clone());
        //            false
        //        } else {
        //            true
        //        }
        //    });

        //    // return the parts that we deleted into the Vram buffer
        //    for range in overlapped_ranges {
        //        self.move_from_rendering_to_vram(&range);
        //    }
        //    self.move_from_vram_to_rendering(&new_range);

        //    self.ranges_in_rendering.push(new_range);

        //    println!("ranges now {:?}", self.ranges_in_rendering);
        //}
    }

    fn get_semi_transparency_blending_params(&self, semi_transparecy_mode: u8) -> () {
        //let color_func = match semi_transparecy_mode & 3 {
        //    0 => BlendingFunction::Addition {
        //        source: LinearBlendingFactor::SourceAlpha,
        //        destination: LinearBlendingFactor::OneMinusSourceAlpha,
        //    },
        //    1 => BlendingFunction::Addition {
        //        source: LinearBlendingFactor::One,
        //        destination: LinearBlendingFactor::SourceAlpha,
        //    },
        //    2 => BlendingFunction::ReverseSubtraction {
        //        source: LinearBlendingFactor::One,
        //        destination: LinearBlendingFactor::SourceAlpha,
        //    },
        //    3 => BlendingFunction::Addition {
        //        source: LinearBlendingFactor::SourceAlpha,
        //        destination: LinearBlendingFactor::OneMinusSourceAlpha,
        //    },
        //    _ => unreachable!(),
        //};

        //Blend {
        //    color: color_func,
        //    // TODO: handle alpha so that it takes the mask value
        //    alpha: BlendingFunction::AlwaysReplace,
        //    constant_value: (1.0, 1.0, 1.0, 1.0),
        //}
    }
}

impl GpuContext {
    pub fn write_vram_block(&mut self, block_range: (Range<u32>, Range<u32>), block: &[u16]) {
        // todo!()
        // cannot write outside range
        //assert!(block_range.0.end <= 1024);
        //assert!(block_range.1.end <= 512);

        //let whole_size = block_range.0.len() * block_range.1.len();
        //assert_eq!(block.len(), whole_size);

        //let (drawing_left, drawing_top) = self.drawing_area_top_left;
        //let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;
        //let drawing_range = (
        //    drawing_left..(drawing_right + 1),
        //    drawing_top..(drawing_bottom + 1),
        //);

        //// add the current drawing area to rendering range
        ////
        //// if we are copying a block into a rendering range, then just blit
        //// directly into it
        //self.add_to_rendering_range(drawing_range);

        //if self.is_block_in_rendering(&block_range) {
        //    let (x_range, y_range) = block_range;
        //    let width = x_range.len() as u32;
        //    let height = y_range.len() as u32;

        //    // reverse on y axis
        //    let block: Vec<_> = block
        //        .chunks(width as usize)
        //        .rev()
        //        .flat_map(|row| row.iter())
        //        .cloned()
        //        .collect();

        //    self.drawing_texture.write(
        //        Rect {
        //            left: x_range.start,
        //            bottom: to_gl_bottom(y_range.start, height),
        //            width,
        //            height,
        //        },
        //        RawImage2d {
        //            data: Cow::Borrowed(block.as_ref()),
        //            width,
        //            height,
        //            format: ClientFormat::U1U5U5U5Reversed,
        //        },
        //    );
        //} else {
        //    self.vram.write_block(&block_range, block);
        //    self.update_texture_buffer();
        //}
    }

    pub fn read_vram_block(&mut self, block_range: &(Range<u32>, Range<u32>)) -> Vec<u16> {
        vec![0; block_range.0.len() * block_range.1.len()]
        //todo!()
        // cannot read outside range
        //assert!(block_range.0.end <= 1024);
        //assert!(block_range.1.end <= 512);

        //if self.is_block_in_rendering(block_range) {
        //    let (x_range, y_range) = block_range;
        //    let x_size = x_range.len() as u32;
        //    let y_size = y_range.len() as u32;

        //    let tex = Texture2d::empty_with_mipmaps(
        //        &self.gl_context,
        //        MipmapsOption::NoMipmap,
        //        x_size,
        //        y_size,
        //    )
        //    .unwrap();
        //    self.drawing_texture.as_surface().blit_color(
        //        &Rect {
        //            left: x_range.start,
        //            bottom: to_gl_bottom(y_range.start, y_size),
        //            width: x_size,
        //            height: y_size,
        //        },
        //        &tex.as_surface(),
        //        &BlitTarget {
        //            left: 0,
        //            bottom: 0,
        //            width: x_size as i32,
        //            height: y_size as i32,
        //        },
        //        MagnifySamplerFilter::Nearest,
        //    );

        //    let pixel_buffer: Vec<_> = tex.read();

        //    let block = pixel_buffer
        //        .iter()
        //        // reverse, as its from bottom to top
        //        .rev()
        //        .flatten()
        //        .map(|pixel| gl_pixel_to_u16(pixel))
        //        .collect::<Vec<_>>();

        //    block
        //} else {
        //    self.vram.read_block(block_range, false)
        //}
    }

    pub fn fill_color(&mut self, top_left: (u32, u32), size: (u32, u32), color: (u8, u8, u8)) {
        todo!()
        //let mut x_range = (top_left.0)..(top_left.0 + size.0);
        //let mut y_range = (top_left.1)..(top_left.1 + size.1);

        //if x_range.end >= 1024 {
        //    self.fill_color((0, top_left.1), (x_range.end - 1024 + 1, size.1), color);
        //    x_range.end = 1023;
        //}
        //if y_range.end >= 512 {
        //    self.fill_color((top_left.0, 0), (size.0, y_range.end - 512 + 1), color);
        //    y_range.end = 511;
        //}

        //let block_range = (x_range, y_range);

        //if self.is_block_in_rendering(&block_range) {
        //    self.drawing_texture.as_surface().clear(
        //        Some(&Rect {
        //            left: top_left.0,
        //            bottom: to_gl_bottom(top_left.1, size.1),
        //            width: size.0,
        //            height: size.1,
        //        }),
        //        Some((
        //            color.0 as f32 / 255.0,
        //            color.1 as f32 / 255.0,
        //            color.2 as f32 / 255.0,
        //            0.0,
        //        )),
        //        false,
        //        None,
        //        None,
        //    );
        //} else {
        //    let color = gl_pixel_to_u16(&(color.0, color.1, color.2, 0));
        //    let size = block_range.0.len() * block_range.1.len();

        //    self.vram.write_block(&block_range, &vec![color; size]);
        //    self.update_texture_buffer();
        //}
    }

    pub fn update_texture_buffer(&mut self) {
        //self.texture_buffer
        //    .main_level()
        //    .raw_upload_from_pixel_buffer(self.vram.data.as_slice(), 0..1024, 0..512, 0..1);
    }

    pub fn draw_polygon(
        &mut self,
        vertices: &[DrawingVertex],
        mut _texture_params: DrawingTextureParams,
        textured: bool,
        _texture_blending: bool,
        _semi_transparent: bool,
    ) {
        assert!(!textured);

        // copy vertecies
        //{
        //    let mut v_write = self.vertex_buffer.write().unwrap();
        //    v_write.copy_from_slice(vertices);
        //}
        let vertex_buffer = CpuAccessibleBuffer::from_iter(
            self.device.clone(),
            BufferUsage::all(),
            false,
            vertices.into_iter().cloned(),
        )
        .unwrap();

        let (drawing_left, drawing_top) = self.drawing_area_top_left;
        let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;

        let left = drawing_left as f32;
        let top = drawing_top as f32;
        let height = (drawing_bottom - drawing_top + 1) as f32;
        let width = (drawing_right - drawing_left + 1) as f32;

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                self.device.clone(),
                self.queue.family(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        builder
            .begin_render_pass(
                self.render_image_framebuffer.clone(),
                SubpassContents::Inline,
                [ClearValue::None],
            )
            .unwrap()
            .set_viewport(
                0,
                [Viewport {
                    origin: [left, top],
                    dimensions: [width, height],
                    depth_range: 0.0..1.0,
                }],
            )
            .bind_pipeline_graphics(self.pipeline.clone())
            .bind_vertex_buffers(0, vertex_buffer.clone())
            .draw(vertices.len() as u32, 1, 0, 0)
            .unwrap()
            .end_render_pass()
            .unwrap();

        let command_buffer = builder.build().unwrap();

        self.gpu_future = Some(
            self.gpu_future
                .take()
                .unwrap()
                .then_execute(self.queue.clone(), command_buffer)
                .unwrap()
                .then_signal_fence_and_flush()
                .unwrap()
                .boxed(),
        );

        //if textured {
        //    if !self.allow_texture_disable {
        //        texture_params.texture_disable = false;
        //    }
        //    self.update_gpu_stat_from_texture_params(&texture_params);

        //    // if the texure we can using is inside `rendering`, bring it back
        //    // to `vram` and `texture_buffer`
        //    //
        //    // 0 => 64,
        //    // 1 => 128,
        //    // 2 => 256,
        //    let row_size = 64 * (1 << texture_params.tex_page_color_mode);
        //    let texture_block = (
        //        texture_params.tex_page_base[0]..texture_params.tex_page_base[0] + row_size,
        //        texture_params.tex_page_base[1]..texture_params.tex_page_base[1] + 256,
        //    );
        //    if self.is_block_in_rendering(&texture_block) {
        //        self.move_from_rendering_to_vram(&texture_block);
        //    }
        //}

        //// TODO: if its textured, make sure the textures are not in rendering
        ////  ranges and are updated in the texture buffer

        //let (drawing_left, drawing_top) = self.drawing_area_top_left;
        //let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;

        //let drawing_range = (
        //    drawing_left..(drawing_right + 1),
        //    drawing_top..(drawing_bottom + 1),
        //);

        //self.add_to_rendering_range(drawing_range);

        //let left = drawing_left;
        //let top = drawing_top;
        //let height = drawing_bottom - drawing_top + 1;
        //let width = drawing_right - drawing_left + 1;
        //let bottom = to_gl_bottom(top, height);

        //let semi_transparency_mode = if textured {
        //    texture_params.semi_transparency_mode
        //} else {
        //    self.gpu_stat.semi_transparency_mode()
        //};
        //let blend = self.get_semi_transparency_blending_params(semi_transparency_mode);

        //let draw_params = glium::DrawParameters {
        //    viewport: Some(glium::Rect {
        //        left,
        //        bottom,
        //        width,
        //        height,
        //    }),
        //    blend,
        //    color_mask: (true, true, true, false),
        //    ..Default::default()
        //};

        //let full_index_list = &[0u16, 1, 2, 1, 2, 3];
        //let index_list = if vertices.len() == 4 {
        //    &full_index_list[..]
        //} else {
        //    &full_index_list[..3]
        //};

        //let vertex_buffer = VertexBuffer::new(&self.gl_context, vertices).unwrap();
        //let index_buffer =
        //    IndexBuffer::new(&self.gl_context, PrimitiveType::TrianglesList, index_list).unwrap();

        //let uniforms = uniform! {
        //    offset: self.drawing_offset,
        //    texture_flip_x: texture_params.texture_flip.0,
        //    texture_flip_y: texture_params.texture_flip.1,
        //    is_textured: textured,
        //    is_texture_blended: texture_blending,
        //    semi_transparency_mode: semi_transparency_mode,
        //    semi_transparent: semi_transparent,
        //    tex: self.texture_buffer.sampled(),
        //    tex_page_base: texture_params.tex_page_base,
        //    tex_page_color_mode: texture_params.tex_page_color_mode,
        //    clut_base: texture_params.clut_base,
        //    drawing_top_left: [left, top],
        //    drawing_size: [width, height],
        //};

        //let mut texture_target = self.drawing_texture.as_surface();
        //texture_target
        //    .draw(
        //        &vertex_buffer,
        //        &index_buffer,
        //        &self.program,
        //        &uniforms,
        //        &draw_params,
        //    )
        //    .unwrap();
    }

    pub fn blit_to_front<D, IF>(&mut self, dest_image: Arc<D>, full_vram: bool, in_future: IF)
    where
        D: ImageAccess + 'static,
        IF: GpuFuture,
    {
        self.gpu_future.as_mut().unwrap().cleanup_finished();

        let (left, top, width, height) = if full_vram {
            (0, 0, 1024, 512)
        } else {
            (
                self.vram_display_area_start.0 as i32,
                self.vram_display_area_start.1 as i32,
                self.gpu_stat.horizontal_resolution() as i32,
                self.gpu_stat.vertical_resolution() as i32,
            )
        };

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                self.device.clone(),
                self.queue.family(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        // TODO: add blit commands
        builder
            .clear_color_image(dest_image.clone(), ClearValue::Float([0.0, 0.0, 0.0, 0.0]))
            .unwrap()
            .blit_image(
                self.render_image.clone(),
                [left, top, 0],
                [width, height, 1],
                0,
                0,
                dest_image.clone(),
                [0, 0, 0],
                [
                    dest_image.dimensions().width() as i32,
                    dest_image.dimensions().height() as i32,
                    1,
                ],
                0,
                0,
                1,
                Filter::Linear,
            )
            .unwrap();

        let command_buffer = builder.build().unwrap();

        self.gpu_future
            .take()
            .unwrap()
            .join(in_future)
            .then_execute(self.queue.clone(), command_buffer)
            .unwrap()
            .then_signal_fence_and_flush()
            .unwrap()
            // TODO: don't wait, make it all async
            .wait(None)
            .unwrap();

        // reset future since we are waiting
        self.gpu_future = Some(sync::now(self.device.clone()).boxed());

        //let (left, top) = self.vram_display_area_start;
        //let width = self.gpu_stat.horizontal_resolution();
        //let height = self.gpu_stat.vertical_resolution();
        //let bottom = to_gl_bottom(top, height);

        //let src_rect = if full_vram {
        //    Rect {
        //        left: 0,
        //        bottom: 0,
        //        width: 1024,
        //        height: 512,
        //    }
        //} else {
        //    Rect {
        //        left,
        //        bottom,
        //        width,
        //        height,
        //    }
        //};

        //let (target_w, target_h) = s.get_dimensions();

        //self.drawing_texture.as_surface().blit_color(
        //    &src_rect,
        //    s,
        //    &BlitTarget {
        //        left: 0,
        //        bottom: 0,
        //        width: target_w as i32,
        //        height: target_h as i32,
        //    },
        //    MagnifySamplerFilter::Nearest,
        //);
    }
}
