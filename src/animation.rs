#[cfg(feature = "bincode")]
use bincode::{Decode, Encode};
use glam::{Quat, Vec3, Vec4};
use std::mem;
use std::path::Path;
use std::simd::prelude::*;
use std::simd::*;

use crate::archive::{ArchiveReader, ArchiveTag, ArchiveVersion, IArchive};
use crate::math::{as_f32x4, as_i32x4, f16_to_f32, simd_f16_to_f32, SoaFloat3, SoaQuaternion};
use crate::OzzError;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bincode", derive(Encode, Decode))]
pub struct Float3Key {
    pub ratio: f32,
    pub track: u16,
    pub value: [u16; 3],
}

impl Float3Key {
    pub fn new(ratio: f32, track: u16, value: [u16; 3]) -> Float3Key {
        return Float3Key { ratio, track, value };
    }

    pub fn decompress(&self) -> Vec3 {
        return Vec3::new(
            f16_to_f32(self.value[0]),
            f16_to_f32(self.value[1]),
            f16_to_f32(self.value[2]),
        );
    }

    pub fn simd_decompress(k0: &Float3Key, k1: &Float3Key, k2: &Float3Key, k3: &Float3Key, soa: &mut SoaFloat3) {
        soa.x = simd_f16_to_f32([k0.value[0], k1.value[0], k2.value[0], k3.value[0]]);
        soa.y = simd_f16_to_f32([k0.value[1], k1.value[1], k2.value[1], k3.value[1]]);
        soa.z = simd_f16_to_f32([k0.value[2], k1.value[2], k2.value[2], k3.value[2]]);
    }
}

impl ArchiveReader<Float3Key> for Float3Key {
    fn read(archive: &mut IArchive) -> Result<Float3Key, OzzError> {
        let ratio: f32 = archive.read()?;
        let track: u16 = archive.read()?;
        let value: [u16; 3] = [archive.read()?, archive.read()?, archive.read()?];
        return Ok(Float3Key { ratio, track, value });
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "bincode", derive(Encode, Decode))]
pub struct QuaternionKey {
    pub ratio: f32,
    // track: 13 => The track this key frame belongs to.
    // largest: 2 => The largest component of the quaternion.
    // sign: 1 => The sign of the largest component. 1 for negative.
    bit_field: u16,
    pub value: [i16; 3], // The quantized value of the 3 smallest components.
}

impl QuaternionKey {
    pub fn new(ratio: f32, bit_field: u16, value: [i16; 3]) -> QuaternionKey {
        return QuaternionKey {
            ratio,
            bit_field,
            value,
        };
    }

    pub fn ratio(&self) -> f32 {
        return self.ratio;
    }

    pub fn track(&self) -> u16 {
        return self.bit_field >> 3;
    }

    pub fn largest(&self) -> u16 {
        return (self.bit_field & 0x6) >> 1;
    }

    pub fn sign(&self) -> u16 {
        return self.bit_field & 0x1;
    }

    pub fn decompress(&self) -> Quat {
        const MAPPING: [[usize; 4]; 4] = [[0, 0, 1, 2], [0, 0, 1, 2], [0, 1, 0, 2], [0, 1, 2, 0]];

        let mask = &MAPPING[self.largest() as usize];
        let mut cmp_keys = [
            self.value[mask[0]],
            self.value[mask[1]],
            self.value[mask[2]],
            self.value[mask[3]],
        ];
        cmp_keys[self.largest() as usize] = 0;

        const INT_2_FLOAT: f32 = 1.0f32 / (32767.0f32 * core::f32::consts::SQRT_2);
        let mut cpnt = Vec4::new(
            (cmp_keys[0] as f32) * INT_2_FLOAT,
            (cmp_keys[1] as f32) * INT_2_FLOAT,
            (cmp_keys[2] as f32) * INT_2_FLOAT,
            (cmp_keys[3] as f32) * INT_2_FLOAT,
        );

        let dot = cpnt[0] * cpnt[0] + cpnt[1] * cpnt[1] + cpnt[2] * cpnt[2] + cpnt[3] * cpnt[3];
        let ww0 = f32::max(1e-16f32, 1f32 - dot);
        let w0 = ww0.sqrt();
        let restored = if self.sign() == 0 { w0 } else { -w0 };

        cpnt[self.largest() as usize] = restored;
        return Quat::from_vec4(cpnt);
    }

    #[rustfmt::skip]
    pub fn simd_decompress(
        k0: &QuaternionKey,
        k1: &QuaternionKey,
        k2: &QuaternionKey,
        k3: &QuaternionKey,
        soa: &mut SoaQuaternion,
    ) {
        const INT_2_FLOAT: f32x4 = f32x4::from_array([1.0f32 / (32767.0f32 * core::f32::consts::SQRT_2); 4]);

        const ONE: f32x4 = f32x4::from_array([1.0f32; 4]);
        const SMALL: f32x4 = f32x4::from_array([1e-16f32; 4]);

        const MASK_F000:i32x4 = i32x4::from_array([-1i32, 0, 0, 0]);
        const MASK_0F00:i32x4 = i32x4::from_array([0, -1i32, 0, 0]);
        const MASK_00F0:i32x4 = i32x4::from_array([0, 0, -1i32, 0]);
        const MASK_000F:i32x4 = i32x4::from_array([0, 0, 0, -1i32]);

        const MAPPING: [[usize; 4]; 4] = [[0, 0, 1, 2], [0, 0, 1, 2], [0, 1, 0, 2], [0, 1, 2, 0]];

        let m0 = &MAPPING[k0.largest() as usize];
        let m1 = &MAPPING[k1.largest() as usize];
        let m2 = &MAPPING[k2.largest() as usize];
        let m3 = &MAPPING[k3.largest() as usize];

        let mut cmp_keys: [[f32; 4]; 4] = [
            [ k0.value[m0[0]] as f32, k1.value[m1[0]] as f32, k2.value[m2[0]] as f32, k3.value[m3[0]] as f32 ],
            [ k0.value[m0[1]] as f32, k1.value[m1[1]] as f32, k2.value[m2[1]] as f32, k3.value[m3[1]] as f32 ],
            [ k0.value[m0[2]] as f32, k1.value[m1[2]] as f32, k2.value[m2[2]] as f32, k3.value[m3[2]] as f32 ],
            [ k0.value[m0[3]] as f32, k1.value[m1[3]] as f32, k2.value[m2[3]] as f32, k3.value[m3[3]] as f32 ],
        ]; // TODO: simd int to float
        cmp_keys[k0.largest() as usize][0] = 0.0f32;
        cmp_keys[k1.largest() as usize][1] = 0.0f32;
        cmp_keys[k2.largest() as usize][2] = 0.0f32;
        cmp_keys[k3.largest() as usize][3] = 0.0f32;

        let mut cpnt = [
            INT_2_FLOAT * f32x4::from_array(cmp_keys[0]),
            INT_2_FLOAT * f32x4::from_array(cmp_keys[1]),
            INT_2_FLOAT * f32x4::from_array(cmp_keys[2]),
            INT_2_FLOAT * f32x4::from_array(cmp_keys[3]),
        ];
        let dot = cpnt[0] * cpnt[0] + cpnt[1] * cpnt[1] + cpnt[2] * cpnt[2] + cpnt[3] * cpnt[3];
        let ww0 = f32x4::simd_max(SMALL, ONE - dot);
        let w0 = ww0 * ww0.recip().sqrt();
        let sign = i32x4::from_array([k0.sign() as i32, k1.sign() as i32, k2.sign() as i32, k3.sign() as i32]) << 31;
        let restored = as_i32x4(w0) | sign;

        cpnt[k0.largest() as usize] = as_f32x4(as_i32x4(cpnt[k0.largest() as usize]) | (restored & MASK_F000));
        cpnt[k1.largest() as usize] = as_f32x4(as_i32x4(cpnt[k1.largest() as usize]) | (restored & MASK_0F00));
        cpnt[k2.largest() as usize] = as_f32x4(as_i32x4(cpnt[k2.largest() as usize]) | (restored & MASK_00F0));
        cpnt[k3.largest() as usize] = as_f32x4(as_i32x4(cpnt[k3.largest() as usize]) | (restored & MASK_000F));

        soa.x = unsafe { mem::transmute(cpnt[0]) };
        soa.y = unsafe { mem::transmute(cpnt[1]) };
        soa.z = unsafe { mem::transmute(cpnt[2]) };
        soa.w = unsafe { mem::transmute(cpnt[3]) };
    }
}

impl ArchiveReader<QuaternionKey> for QuaternionKey {
    fn read(archive: &mut IArchive) -> Result<QuaternionKey, OzzError> {
        let ratio: f32 = archive.read()?;
        let track: u16 = archive.read()?;
        let largest: u8 = archive.read()?;
        let sign: u8 = archive.read()?;
        let bit_field: u16 = ((track & 0x1FFF) << 3) | ((largest as u16 & 0x3) << 1) | (sign as u16 & 0x1);
        let value: [i16; 3] = [archive.read()?, archive.read()?, archive.read()?];
        return Ok(QuaternionKey {
            ratio,
            bit_field,
            value,
        });
    }
}

#[derive(Debug)]
#[cfg_attr(feature = "bincode", derive(Encode, Decode))]
pub struct Animation {
    pub(crate) duration: f32,
    pub(crate) num_tracks: usize,
    pub(crate) name: String,
    pub(crate) translations: Vec<Float3Key>,
    pub(crate) rotations: Vec<QuaternionKey>,
    pub(crate) scales: Vec<Float3Key>,
}

impl ArchiveVersion for Animation {
    fn version() -> u32 {
        return 6;
    }
}

impl ArchiveTag for Animation {
    fn tag() -> &'static str {
        return "ozz-animation";
    }
}

impl ArchiveReader<Animation> for Animation {
    fn read(archive: &mut IArchive) -> Result<Animation, OzzError> {
        if !archive.test_tag::<Self>()? {
            return Err(OzzError::InvalidTag);
        }

        let version = archive.read_version()?;
        if version != Self::version() {
            return Err(OzzError::InvalidVersion);
        }

        let duration: f32 = archive.read()?;
        let num_tracks: i32 = archive.read()?;
        let name_len: i32 = archive.read()?;
        let translation_count: i32 = archive.read()?;
        let rotation_count: i32 = archive.read()?;
        let scale_count: i32 = archive.read()?;

        let name: String = archive.read_string(name_len as usize)?;
        let translations: Vec<Float3Key> = archive.read_vec(translation_count as usize)?;
        let rotations: Vec<QuaternionKey> = archive.read_vec(rotation_count as usize)?;
        let scales: Vec<Float3Key> = archive.read_vec(scale_count as usize)?;

        return Ok(Animation {
            duration,
            num_tracks: num_tracks as usize,
            name,
            translations,
            rotations,
            scales,
        });
    }
}

impl Animation {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Animation, OzzError> {
        let mut archive = IArchive::new(path)?;
        return Animation::read(&mut archive);
    }

    pub fn from_reader(archive: &mut IArchive) -> Result<Animation, OzzError> {
        return Animation::read(archive);
    }
}

impl Animation {
    pub fn duration(&self) -> f32 {
        return self.duration;
    }

    pub fn num_tracks(&self) -> usize {
        return self.num_tracks;
    }

    pub fn num_aligned_tracks(&self) -> usize {
        return (self.num_tracks + 3) & !0x3;
    }

    pub fn num_soa_tracks(&self) -> usize {
        return (self.num_tracks + 3) / 4;
    }

    pub fn name(&self) -> &str {
        return &self.name;
    }

    pub fn translations(&self) -> &[Float3Key] {
        return &self.translations;
    }

    pub fn rotations(&self) -> &[QuaternionKey] {
        return &self.rotations;
    }

    pub fn scales(&self) -> &[Float3Key] {
        return &self.scales;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_float3_key_decompress() {
        let res = Float3Key {
            ratio: 0.0,
            track: 0,
            value: [11405, 34240, 31],
        }
        .decompress();
        assert_eq!(res, Vec3::new(0.0711059570, -8.77380371e-05, 1.84774399e-06));

        let res = Float3Key {
            ratio: 0.0,
            track: 0,
            value: [9839, 1, 0],
        }
        .decompress();
        assert_eq!(res, Vec3::new(0.0251312255859375, 5.960464477539063e-8, 0.0));
    }

    #[test]
    fn test_simd_decompress_float3() {
        let k0 = Float3Key {
            ratio: 0.0,
            track: 0,
            value: [11405, 34240, 31],
        };
        let k1 = Float3Key {
            ratio: 0.0,
            track: 0,
            value: [9839, 1, 0],
        };
        let k2 = Float3Key {
            ratio: 0.0,
            track: 0,
            value: [11405, 34240, 31],
        };
        let k3 = Float3Key {
            ratio: 0.0,
            track: 0,
            value: [9839, 1, 0],
        };
        let mut soa = SoaFloat3::default();
        Float3Key::simd_decompress(&k0, &k1, &k2, &k3, &mut soa);
        assert_eq!(
            soa,
            SoaFloat3 {
                x: f32x4::from_array([0.0711059570, 0.0251312255859375, 0.0711059570, 0.0251312255859375]),
                y: f32x4::from_array([
                    -8.77380371e-05,
                    5.960464477539063e-8,
                    -8.77380371e-05,
                    5.960464477539063e-8
                ]),
                z: f32x4::from_array([1.84774399e-06, 0.0, 1.84774399e-06, 0.0]),
            }
        );
    }

    #[test]
    fn test_quaternion_key_decompress() {
        let quat = QuaternionKey {
            ratio: 0.0,
            bit_field: (3 << 1) | 0,
            value: [396, 409, 282],
        }
        .decompress();
        assert_eq!(
            quat,
            Quat::from_xyzw(
                0.008545618438802194,
                0.008826156417853781,
                0.006085516160965199,
                0.9999060145140845,
            )
        );

        let quat = QuaternionKey {
            ratio: 0.0,
            bit_field: (0 << 1) | 0,
            value: [5256, -14549, 25373],
        }
        .decompress();
        assert_eq!(
            quat,
            Quat::from_xyzw(
                0.767303715540273,
                0.11342366291501094,
                -0.3139651582478109,
                0.5475453955750709,
            )
        );

        let quat = QuaternionKey {
            ratio: 0.0,
            bit_field: (3 << 1) | 0,
            value: [0, 0, -195],
        }
        .decompress();
        assert_eq!(
            quat,
            Quat::from_xyzw(0.00000000, 0.00000000, -0.00420806976, 0.999991119)
        );

        let quat = QuaternionKey {
            ratio: 0.0,
            bit_field: (2 << 1) | 1,
            value: [-23255, -23498, 21462],
        }
        .decompress();
        assert_eq!(
            quat,
            Quat::from_xyzw(-0.501839280, -0.507083178, -0.525850952, 0.463146627)
        );
    }

    #[test]
    fn test_simd_decompress_quaternion() {
        let quat0 = QuaternionKey {
            ratio: 0.0,
            bit_field: (3 << 1) | 0,
            value: [396, 409, 282],
        };
        let quat1 = QuaternionKey {
            ratio: 0.0,
            bit_field: (0 << 1) | 0,
            value: [5256, -14549, 25373],
        };
        let quat2 = QuaternionKey {
            ratio: 0.0,
            bit_field: (3 << 1) | 0,
            value: [0, 0, -195],
        };
        let quat3 = QuaternionKey {
            ratio: 0.0,
            bit_field: (2 << 1) | 1,
            value: [-23255, -23498, 21462],
        };
        let mut soa = SoaQuaternion::default();
        QuaternionKey::simd_decompress(&quat0, &quat1, &quat2, &quat3, &mut soa);
        assert_eq!(
            soa,
            SoaQuaternion {
                x: f32x4::from_array([0.008545618438802194, 0.767303715540273, 0.00000000, -0.501839280]),
                y: f32x4::from_array([0.008826156417853781, 0.11342366291501094, 0.00000000, -0.507083178]),
                z: f32x4::from_array([0.006085516160965199, -0.3139651582478109, -0.00420806976, -0.525850952]),
                w: f32x4::from_array([0.9999060145140845, 0.5475453955750709, 0.999991119, 0.463146627]),
            }
        );
    }

    #[test]
    fn test_read_animation() {
        let mut archive = IArchive::new("./resource/playback/animation.ozz").unwrap();
        let animation = Animation::read(&mut archive).unwrap();

        assert_eq!(animation.duration(), 8.60000038);
        assert_eq!(animation.num_tracks(), 67);
        assert_eq!(animation.name(), "crossarms".to_string());

        let last = animation.translations().len() - 1;
        assert_eq!(animation.translations().len(), 178);
        assert_eq!(animation.translations[0].ratio, 0f32);
        assert_eq!(animation.translations[0].track, 0);
        assert_eq!(animation.translations[0].value, [0, 15400, 43950]);
        assert_eq!(animation.translations[last].ratio, 1f32);
        assert_eq!(animation.translations[last].track, 0);
        assert_eq!(animation.translations[last].value, [3659, 15400, 43933]);

        let last = animation.rotations().len() - 1;
        assert_eq!(animation.rotations().len(), 1678);
        assert_eq!(animation.rotations[0].ratio, 0f32);
        assert_eq!(animation.rotations[0].track(), 0);
        assert_eq!(animation.rotations[0].largest(), 2);
        assert_eq!(animation.rotations[0].sign(), 1);
        assert_eq!(animation.rotations[0].value, [-22775, -23568, 21224]);
        assert_eq!(animation.rotations[last].ratio, 1f32);
        assert_eq!(animation.rotations[last].track(), 63);
        assert_eq!(animation.rotations[last].largest(), 3);
        assert_eq!(animation.rotations[last].sign(), 0);
        assert_eq!(animation.rotations[last].value, [0, 0, -2311]);

        let last = animation.scales().len() - 1;
        assert_eq!(animation.scales().len(), 136);
        assert_eq!(animation.scales()[0].ratio, 0f32);
        assert_eq!(animation.scales()[0].track, 0);
        assert_eq!(animation.scales()[0].value, [15360, 15360, 15360]);
        assert_eq!(animation.scales()[last].ratio, 1f32);
        assert_eq!(animation.scales()[last].track, 67);
        assert_eq!(animation.scales()[last].value, [15360, 15360, 15360]);
    }
}
