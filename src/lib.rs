//! # AVIF image serializer (muxer)
//!
//! ## Usage
//!
//! 1. Compress pixels using an AV1 encoder, such as [rav1e](//lib.rs/rav1e). [libaom](//lib.rs/libaom-sys) works too.
//!
//! 2. Call `avif_serialize::serialize_to_vec(av1_data, None, width, height, 8)`
//!
//! See [cavif](https://github.com/kornelski/cavif-rs) for a complete implementation.

mod boxes;
pub mod constants;
mod writer;

use crate::boxes::*;
use arrayvec::ArrayVec;
use std::io;
// use std::{io, time::SystemTime};

/// Config for the serialization (allows setting advanced image properties).
///
/// See [`Aviffy::new`].
pub struct Aviffy {
    premultiplied_alpha: bool,
    colr: ColrBox,
}

/// Makes an AVIF file given encoded AV1 data (create the data with [`rav1e`](//lib.rs/rav1e))
///
/// `color_av1_data` is already-encoded AV1 image data for the color channels (YUV, RGB, etc.).
/// The color image MUST have been encoded without chroma subsampling AKA YUV444 (`Cs444` in `rav1e`)
/// AV1 handles full-res color so effortlessly, you should never need chroma subsampling ever again.
///
/// Optional `alpha_av1_data` is a monochrome image (`rav1e` calls it "YUV400"/`Cs400`) representing transparency.
/// Alpha adds a lot of header bloat, so don't specify it unless it's necessary.
///
/// `width`/`height` is image size in pixels. It must of course match the size of encoded image data.
/// `depth_bits` should be 8, 10 or 12, depending on how the image was encoded (typically 8).
///
/// Color and alpha must have the same dimensions and depth.
///
/// Data is written (streamed) to `into_output`.
pub fn serialize<W: io::Write>(into_output: W, color_av1_data: &[u8], alpha_av1_data: Option<&[u8]>, width: u32, height: u32, depth_bits: u8, timescale: u32, color_frames: Option<&[FrameInfo]>, alpha_frames: Option<&[FrameInfo]>) -> io::Result<()> {
    Aviffy::new().write(into_output, color_av1_data, alpha_av1_data, width, height, depth_bits, timescale, color_frames, alpha_frames)
}

impl Aviffy {
    #[must_use]
    pub fn new() -> Self {
        Self {
            premultiplied_alpha: false,
            colr: Default::default(),
        }
    }

    /// Set whether image's colorspace uses premultiplied alpha, i.e. RGB channels were multiplied by their alpha value,
    /// so that transparent areas are all black. Image decoders will be instructed to undo the premultiplication.
    ///
    /// Premultiplied alpha images usually compress better and tolerate heavier compression, but
    /// may not be supported correctly by less capable AVIF decoders.
    ///
    /// This just sets the configuration property. The pixel data must have already been processed before compression.
    pub fn premultiplied_alpha(&mut self, is_premultiplied: bool) -> &mut Self {
        self.premultiplied_alpha = is_premultiplied;
        self
    }

    /// If set, must match the AV1 color payload, and will result in `colr` box added to AVIF.
    /// Defaults to BT.601, because that's what Safari assumes when `colr` is missing.
    /// Other browsers are smart enough to read this from the AV1 payload instead.
    pub fn matrix_coefficients(&mut self, matrix_coefficients: constants::MatrixCoefficients) -> &mut Self {
        self.colr.matrix_coefficients = matrix_coefficients;
        self
    }

    /// If set, must match the AV1 color payload, and will result in `colr` box added to AVIF.
    /// Defaults to sRGB.
    pub fn transfer_characteristics(&mut self, transfer_characteristics: constants::TransferCharacteristics) -> &mut Self {
        self.colr.transfer_characteristics = transfer_characteristics;
        self
    }

    /// If set, must match the AV1 color payload, and will result in `colr` box added to AVIF.
    /// Defaults to sRGB/Rec.709.
    pub fn color_primaries(&mut self, color_primaries: constants::ColorPrimaries) -> &mut Self {
        self.colr.color_primaries = color_primaries;
        self
    }

    /// If set, must match the AV1 color payload, and will result in `colr` box added to AVIF.
    /// Defaults to full.
    pub fn full_color_range(&mut self, full_range: bool) -> &mut Self {
        self.colr.full_range_flag = full_range;
        self
    }

    /// Makes an AVIF file given encoded AV1 data (create the data with [`rav1e`](//lib.rs/rav1e))
    ///
    /// `color_av1_data` is already-encoded AV1 image data for the color channels (YUV, RGB, etc.).
    /// The color image MUST have been encoded without chroma subsampling AKA YUV444 (`Cs444` in `rav1e`)
    /// AV1 handles full-res color so effortlessly, you should never need chroma subsampling ever again.
    ///
    /// Optional `alpha_av1_data` is a monochrome image (`rav1e` calls it "YUV400"/`Cs400`) representing transparency.
    /// Alpha adds a lot of header bloat, so don't specify it unless it's necessary.
    ///
    /// `width`/`height` is image size in pixels. It must of course match the size of encoded image data.
    /// `depth_bits` should be 8, 10 or 12, depending on how the image has been encoded in AV1.
    ///
    /// Color and alpha must have the same dimensions and depth.
    ///
    /// Data is written (streamed) to `into_output`.
    pub fn write<W: io::Write>(&self, into_output: W, color_av1_data: &[u8], alpha_av1_data: Option<&[u8]>, width: u32, height: u32, depth_bits: u8, timescale: u32, color_frames: Option<&[FrameInfo]>, alpha_frames: Option<&[FrameInfo]>) -> io::Result<()> {
        self.make_boxes(color_av1_data, alpha_av1_data, width, height, depth_bits, timescale, color_frames, alpha_frames).write(into_output)
    }

    fn make_boxes<'data>(&self, color_av1_data: &'data [u8], alpha_av1_data: Option<&'data [u8]>, width: u32, height: u32, depth_bits: u8, timescale: u32, color_frames: Option<&[FrameInfo]>, alpha_frames: Option<&[FrameInfo]>) -> AvifFile<'data> {
        let mut image_items = ArrayVec::new();
        let mut iloc_items = ArrayVec::new();
        let mut compatible_brands = vec![];
        let mut ipma_entries = ArrayVec::new();
        let mut data_chunks = ArrayVec::new();
        let mut irefs = ArrayVec::new();
        let mut ipco = IpcoBox::new();
        let color_image_id = 1;
        let alpha_image_id = 2;
        const ESSENTIAL_BIT: u8 = 0x80;
        let color_depth_bits = depth_bits;
        let alpha_depth_bits = depth_bits; // Sadly, the spec requires these to match.

        image_items.push(InfeBox {
            id: color_image_id,
            typ: FourCC(*b"av01"),
            name: "Color",
        });
        let ispe_prop = ipco.push(IpcoProp::Ispe(IspeBox { width, height }));
        // Useless bloat
        let pixi_3 = ipco.push(IpcoProp::Pixi(PixiBox {
            channels: 3,
            depth: color_depth_bits,
        }));
        let color_config = Av1CBox {
            seq_profile: if color_depth_bits >= 12 { 2 } else { 1 },
            seq_level_idx_0: 31,
            seq_tier_0: false,
            high_bitdepth: color_depth_bits >= 10,
            twelve_bit: color_depth_bits >= 12,
            monochrome: false,
            chroma_subsampling_x: false,
            chroma_subsampling_y: false,
            chroma_sample_position: 0,
        };
        // This is redundant, but Chrome wants it, and checks that it matches :(
        let av1c_color_prop = ipco.push(IpcoProp::Av1C(color_config));
        let mut prop_ids: ArrayVec<u8, 5> = [ispe_prop, pixi_3, av1c_color_prop | ESSENTIAL_BIT].into_iter().collect();
        // Redundant info, already in AV1
        if self.colr != Default::default() {
            let colr_color_prop = ipco.push(IpcoProp::Colr(self.colr));
            prop_ids.push(colr_color_prop);
        }
        ipma_entries.push(IpmaEntry {
            item_id: color_image_id,
            prop_ids,
        });

        let alpha_config = Av1CBox {
            seq_profile: if alpha_depth_bits >= 12 { 2 } else { 0 },
            seq_level_idx_0: 31,
            seq_tier_0: false,
            high_bitdepth: alpha_depth_bits >= 10,
            twelve_bit: alpha_depth_bits >= 12,
            monochrome: true,
            chroma_subsampling_x: true,
            chroma_subsampling_y: true,
            chroma_sample_position: 0,
        };

        if let Some(alpha_data) = alpha_av1_data {
            image_items.push(InfeBox {
                id: alpha_image_id,
                typ: FourCC(*b"av01"),
                name: "Alpha",
            });
            // So pointless
            let pixi_1 = ipco.push(IpcoProp::Pixi(PixiBox {
                channels: 1,
                depth: alpha_depth_bits,
            }));
            let av1c_alpha_prop = ipco.push(boxes::IpcoProp::Av1C(alpha_config));

            // that's a silly way to add 1 bit of information, isn't it?
            let auxc_prop = ipco.push(IpcoProp::AuxC(AuxCBox {
                urn: "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha",
            }));
            irefs.push(IrefBox {
                entry: IrefEntryBox {
                    from_id: alpha_image_id,
                    to_id: color_image_id,
                    typ: FourCC(*b"auxl"),
                },
            });
            if self.premultiplied_alpha {
                irefs.push(IrefBox {
                    entry: IrefEntryBox {
                        from_id: color_image_id,
                        to_id: alpha_image_id,
                        typ: FourCC(*b"prem"),
                    },
                });
            }
            ipma_entries.push(IpmaEntry {
                item_id: alpha_image_id,
                prop_ids: [ispe_prop, pixi_1, av1c_alpha_prop | ESSENTIAL_BIT, auxc_prop].into_iter().collect(),
            });

            // Use interleaved color and alpha, with alpha first.
            // Makes it possible to display partial image.
            iloc_items.push(IlocItem {
                id: color_image_id,
                extents: [
                    IlocExtent {
                        offset: IlocOffset::Relative(alpha_data.len()),
                        len: color_av1_data.len(),
                    },
                ].into(),
            });
            iloc_items.push(IlocItem {
                id: alpha_image_id,
                extents: [
                    IlocExtent {
                        offset: IlocOffset::Relative(0),
                        len: alpha_data.len(),
                    },
                ].into(),
            });
            data_chunks.push(alpha_data);
            data_chunks.push(color_av1_data);
        } else {
            iloc_items.push(IlocItem {
                id: color_image_id,
                extents: [
                    IlocExtent {
                        offset: IlocOffset::Relative(0),
                        len: color_av1_data.len(),
                    },
                ].into(),
            });
            data_chunks.push(color_av1_data);
        };

        let mut moov_box: Option<MoovBox> = None;
        if let Some(_color_frames) = color_frames {
            /*let now = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                Ok(n) => n.as_secs() + 2082844800, // Seconds since 1904-01-01
                Err(_) => 0
            };*/
            let now = 0;
            let mut media_duration = 0;
            for frame in _color_frames {
                media_duration += frame.duration_in_timescales;
            }

            let mut stts_sample_delta: Vec<ArrayVec<u32, 2>> = vec![];
            let mut sample_count: u32 = 0;
            let mut sync_sample_count: u32 = 0;
            let mut sample_number: Vec<u32> = vec![];
            for i in 0.._color_frames.len() {
                if _color_frames[i].sync {
                    sync_sample_count += 1;
                    sample_number.push((i + 1) as u32)
                }

                sample_count += 1;
                if i < (_color_frames.len() - 1) {
                    if _color_frames[i].duration_in_timescales == _color_frames[i + 1].duration_in_timescales {
                        continue;
                    }
                }

                let mut sample: ArrayVec<u32, 2> = ArrayVec::new();
                sample.push(sample_count);
                sample.push(_color_frames[i].duration_in_timescales as u32);
                stts_sample_delta.push(sample);

                sample_count = 0;
            }

            let mut stss_box: Option<StssBox> = None;
            if sync_sample_count != _color_frames.len() as u32 {
                stss_box = Some(StssBox { entry_count: sync_sample_count, sample_number: sample_number })
            }


            moov_box = Some(MoovBox {
                mvhd: MvhdBox {
                    creation_time: now,
                    modification_time: now,
                    timescale: timescale,
                    duration: u64::MAX, // Infinite Repetition
                    next_track_id: match alpha_frames {
                        Some(_) => 2,
                        _ => 1
                    }
                },
                tracks: vec![
                    TrakBox{
                        tkhd: TkhdBox {
                            creation_time: now,
                            modification_time: now,
                            track_id: 1,
                            duration: u64::MAX, // Infinite Repetition
                            width: width << 16, 
                            height: height << 16
                        },
                        tref: None, // TODO: implement
                        /*meta: Some(MetaBox {
                            hdlr: HdlrBox { handler_type: FourCC(*b"pict")},
                            iinf: IinfBox { items: image_items.clone() },
                            pitm: PitmBox(color_image_id),
                            iloc: IlocBox { items: iloc_items.clone() },
                            iprp: IprpBox {
                                ipco: ipco.clone(),
                                // It's not enough to define these properties,
                                // they must be assigned to the image
                                ipma: IpmaBox {
                                    entries: ipma_entries.clone(),
                                },
                            },
                            iref: irefs.clone(),
                        }),*/
                        meta: None,
                        mdia: MdiaBox {
                            mdhd: MdhdBox {
                                creation_time: now,
                                modification_time: now,
                                timescale: timescale,
                                duration: media_duration
                            },
                            hdlr: HdlrBox { handler_type: FourCC(*b"pict"), name: "avifser" },
                            minf: MinfBox {
                                vmhd: VmhdBox {},
                                dinf: DinfBox {
                                    dref: DrefBox { url: UrlBox {} }
                                },
                                stbl: StblBox {
                                    stsd: StsdBox {
                                        entry: SampleEntryBox {
                                            typ: FourCC(*b"av01"),
                                            width: width as u16,
                                            height: height as u16,
                                            config: color_config,
                                            colr: Some(self.colr.clone()),
                                            ccst: CcstBox {},
                                            auxi: None
                                        }
                                    },
                                    stts: SttsBox {
                                        sample_delta: stts_sample_delta
                                    },
                                    stsc: StscBox {
                                        samples_per_chunk: _color_frames.len() as u32
                                    },
                                    stsz: StszBox {
                                        sample_count: _color_frames.len() as u32,
                                        entry_size: _color_frames.iter().map(|x| x.size).collect::<Vec<u32>>()
                                    },
                                    stco: StcoBox { chunk_offset: 1 },
                                    stss: stss_box
                                }
                            }
                        }
                    }
                ]
            });
            if let Some(_alpha_frames) = alpha_frames {
                let mut alpha_stts_sample_delta: Vec<ArrayVec<u32, 2>> = vec![];
                let mut alpha_sample_count: u32 = 0;
                let mut alpha_sync_sample_count: u32 = 0;
                let mut alpha_sample_number: Vec<u32> = vec![];
                for i in 0.._alpha_frames.len() {
                    if _alpha_frames[i].sync {
                        alpha_sync_sample_count += 1;
                        alpha_sample_number.push((i + 1) as u32)
                    }

                    alpha_sample_count += 1;
                    if i < (_alpha_frames.len() - 1) {
                        if _alpha_frames[i].duration_in_timescales == _alpha_frames[i + 1].duration_in_timescales {
                            continue;
                        }
                    }

                    let mut sample: ArrayVec<u32, 2> = ArrayVec::new();
                    sample.push(alpha_sample_count);
                    sample.push(_alpha_frames[i].duration_in_timescales as u32);
                    alpha_stts_sample_delta.push(sample);

                    alpha_sample_count = 0;
                }

                let mut alpha_stss_box: Option<StssBox> = None;
                if alpha_sync_sample_count != _alpha_frames.len() as u32 {
                    alpha_stss_box = Some(StssBox { entry_count: alpha_sync_sample_count, sample_number: alpha_sample_number })
                }

                moov_box.as_mut().unwrap().tracks.push(TrakBox{
                    tkhd: TkhdBox {
                        creation_time: now,
                        modification_time: now,
                        track_id: 2,
                        duration: u64::MAX, // Infinite Repetition
                        width: width << 16, 
                        height: height << 16
                    },
                    tref:Some(TrefBox {
                        ref_type: ReftypeBox {
                            typ: FourCC(*b"auxl"),
                            to_id: 1
                        }
                    }),
                    meta: None,
                    mdia: MdiaBox {
                        mdhd: MdhdBox {
                            creation_time: now,
                            modification_time: now,
                            timescale: timescale,
                            duration: media_duration
                        },
                        hdlr: HdlrBox { handler_type: FourCC(*b"auxv"), name: "avifser" },
                        minf: MinfBox {
                            vmhd: VmhdBox {},
                            dinf: DinfBox {
                                dref: DrefBox { url: UrlBox {} }
                            },
                            stbl: StblBox {
                                stsd: StsdBox {
                                    entry: SampleEntryBox {
                                        typ: FourCC(*b"av01"),
                                        width: width as u16,
                                        height: height as u16,
                                        config: alpha_config,
                                        colr: None,
                                        ccst: CcstBox {},
                                        auxi: Some(AuxiBox { aux_track_type: "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha" })
                                    }
                                },
                                stts: SttsBox {
                                    sample_delta: alpha_stts_sample_delta
                                },
                                stsc: StscBox {
                                    samples_per_chunk: _alpha_frames.len() as u32
                                },
                                stsz: StszBox {
                                    sample_count: _alpha_frames.len() as u32,
                                    entry_size: _alpha_frames.iter().map(|x| x.size).collect::<Vec<u32>>()
                                },
                                stco: StcoBox { chunk_offset: 1 },
                                stss: alpha_stss_box
                            }
                        }
                    }
                });
            }
        }

        compatible_brands.push(FourCC(*b"avif"));
        match moov_box {
            Some(_) => compatible_brands.push(FourCC(*b"avis")),
            _ => ()
        }
        compatible_brands.push(FourCC(*b"mif1"));
        compatible_brands.push(FourCC(*b"miaf"));
        AvifFile {
            ftyp: FtypBox {
                major_brand: match moov_box {
                    Some(_) => FourCC(*b"avis"),
                    _ => FourCC(*b"avif")
                },
                minor_version: 0,
                compatible_brands,
            },
            meta: MetaBox {
                hdlr: HdlrBox { handler_type: FourCC(*b"pict"), name: "avifser" },
                iinf: IinfBox { items: image_items },
                pitm: PitmBox(color_image_id),
                iloc: IlocBox { items: iloc_items },
                iprp: IprpBox {
                    ipco,
                    // It's not enough to define these properties,
                    // they must be assigned to the image
                    ipma: IpmaBox {
                        entries: ipma_entries,
                    },
                },
                iref: irefs,
            },
            moov: moov_box,
            // Here's the actual data. If HEIF wasn't such a kitchen sink, this
            // would have been the only data this file needs.
            mdat: MdatBox {
                data_chunks,
            },
        }
    }

    #[must_use] pub fn to_vec(&self, color_av1_data: &[u8], alpha_av1_data: Option<&[u8]>, width: u32, height: u32, depth_bits: u8, timescale: u32, color_frames: Option<&[FrameInfo]>, alpha_frames: Option<&[FrameInfo]>) -> Vec<u8> {
        let mut out = Vec::with_capacity(color_av1_data.len() + alpha_av1_data.map_or(0, |a| a.len()) + 410);
        self.write(&mut out, color_av1_data, alpha_av1_data, width, height, depth_bits, timescale, color_frames, alpha_frames).unwrap(); // Vec can't fail
        out
    }
}

/// See [`serialize`] for description. This one makes a `Vec` instead of using `io::Write`.
#[must_use] pub fn serialize_to_vec(color_av1_data: &[u8], alpha_av1_data: Option<&[u8]>, width: u32, height: u32, depth_bits: u8, timescale: u32, color_frames: Option<&[FrameInfo]>, alpha_frames: Option<&[FrameInfo]>) -> Vec<u8> {
    Aviffy::new().to_vec(color_av1_data, alpha_av1_data, width, height, depth_bits, timescale, color_frames, alpha_frames)
}

pub struct FrameInfo {
    pub duration_in_timescales: u64,
    pub sync: bool,
    pub size: u32,
}

#[test]
fn test_roundtrip_parse_mp4() {
    let test_img = b"av12356abc";
    let avif = serialize_to_vec(test_img, None, 10, 20, 8, 1, None, None);

    let ctx = mp4parse::read_avif(&mut avif.as_slice(), mp4parse::ParseStrictness::Normal).unwrap();

    assert_eq!(&test_img[..], ctx.primary_item_coded_data());
}

#[test]
fn test_roundtrip_parse_mp4_alpha() {
    let test_img = b"av12356abc";
    let test_a = b"alpha";
    let avif = serialize_to_vec(test_img, Some(test_a), 10, 20, 8, 1, None, None);

    let ctx = mp4parse::read_avif(&mut avif.as_slice(), mp4parse::ParseStrictness::Normal).unwrap();

    assert_eq!(&test_img[..], ctx.primary_item_coded_data());
    assert_eq!(&test_a[..], ctx.alpha_item_coded_data());
}

#[test]
fn test_roundtrip_parse_avif() {
    let test_img = [1,2,3,4,5,6];
    let test_alpha = [77,88,99];
    let avif = serialize_to_vec(&test_img, Some(&test_alpha), 10, 20, 8, 1, None, None);

    let ctx = avif_parse::read_avif(&mut avif.as_slice()).unwrap();

    assert_eq!(&test_img[..], ctx.primary_item.as_slice());
    assert_eq!(&test_alpha[..], ctx.alpha_item.as_deref().unwrap());
}

#[test]
fn test_roundtrip_parse_avif_colr() {
    let test_img = [1,2,3,4,5,6];
    let test_alpha = [77,88,99];
    let avif = Aviffy::new()
        .matrix_coefficients(constants::MatrixCoefficients::Bt709)
        .to_vec(&test_img, Some(&test_alpha), 10, 20, 8, 1, None, None);

    let ctx = avif_parse::read_avif(&mut avif.as_slice()).unwrap();

    assert_eq!(&test_img[..], ctx.primary_item.as_slice());
    assert_eq!(&test_alpha[..], ctx.alpha_item.as_deref().unwrap());
}

#[test]
fn premultiplied_flag() {
    let test_img = [1,2,3,4];
    let test_alpha = [55,66,77,88,99];
    let avif = Aviffy::new().premultiplied_alpha(true).to_vec(&test_img, Some(&test_alpha), 5, 5, 8, 1, None, None);

    let ctx = avif_parse::read_avif(&mut avif.as_slice()).unwrap();

    assert!(ctx.premultiplied_alpha);
    assert_eq!(&test_img[..], ctx.primary_item.as_slice());
    assert_eq!(&test_alpha[..], ctx.alpha_item.as_deref().unwrap());
}
