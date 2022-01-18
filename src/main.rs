use clap::Parser;
use csv::Writer;
use image::{
    imageops::crop, imageops::grayscale, imageops::invert, imageops::resize, imageops::FilterType,
    DynamicImage, GenericImageView, GrayImage, Luma, SubImage,
};
use nalgebra::{vector, EuclideanNorm, Norm};
use ndarray::{Array, Array1};
use rayon::prelude::*;
use std::{cmp, f32::consts::PI, ops::RangeInclusive, path::PathBuf};

#[derive(Debug, Clone, Parser)]
#[clap(author, version, about, long_about = None)]
struct Opt {
    #[clap(parse(from_os_str))]
    /// Path to the target image
    path: PathBuf,
    #[clap(short, long, default_value = "500")]
    /// Number of pins on the loom
    pins: usize,
    #[clap(short, long, default_value = "1000")]
    /// Maximum number of threads used
    threads: usize,
    #[clap(short, long)]
    /// Desired radius of the output image in pixels. Should be no greater than l = min(input_width, input_height). Defaults to l otherwise, or if omitted
    radius: Option<u32>,
    #[clap(short, long, parse(from_os_str))]
    /// Path to output image. Defaults to the parent directory of the input image
    output: Option<PathBuf>,
    #[clap(long)]
    /// Save thread information to a CSV
    #[clap(group = "use_csv")]
    csv: bool,
    #[clap(long, requires = "use_csv")]
    /// Skip image generation
    no_img: bool,
    #[clap(long, requires = "use_csv")]
    /// Write the pixel-coordinates of each ends of a thread to the CSV instead of pin numbers
    write_coords: bool,
    #[clap(long, requires = "use_csv")]
    /// Include a CSV header line. If --write-coords then the header line is `x1, y1, x2, y2`, otherwise `pins`
    header: bool,
}

fn main() {
    let opt = Opt::parse();

    let img = image::open(&opt.path).expect("Couldn't load target image");

    let min_edge = img.width().min(img.height());
    let radius = opt.radius.map_or(min_edge, |radius| radius.min(min_edge));
    let length = radius * 2 + 1;

    let preprocessed = preprocess(img, radius, length);

    let thread_coords = thread(preprocessed, radius, &opt);

    let prefix = format!(
        "{}_{}_{}",
        opt.path.file_stem().unwrap().to_str().unwrap(),
        opt.pins,
        opt.threads
    );
    let mut outfile = opt.clone().output.unwrap_or(opt.clone().path);

    if !opt.no_img {
        write_img(&mut outfile, &prefix, &thread_coords, length);
    }

    if opt.csv {
        write_csv(&mut outfile, &prefix, &thread_coords, &opt);
    }
}

fn thread(mut img: GrayImage, radius: u32, opt: &Opt) -> Vec<(Array1<f64>, Array1<f64>, usize)> {
    let num_pins = opt.pins + 1;
    let loom = Array::linspace(0., 2. * PI, num_pins)
        .into_iter()
        .map(|alpha| {
            vector![
                f64::from(radius) * f64::from(1. + alpha.cos()),
                f64::from(radius) * f64::from(1. + alpha.sin())
            ]
        })
        .collect::<Vec<_>>();

    let mut threads = Vec::with_capacity(opt.threads);
    let mut prev_pins = [0; 2];

    for _ in 0..opt.threads {
        let prev_pin = prev_pins[1];
        let prev_pos = &loom[prev_pin];

        let best = loom[prev_pin..]
            .par_iter()
            .chain(&loom[..prev_pin])
            .enumerate()
            .filter_map(|(i, next_pos)| {
                let next_pin = (prev_pin + i) % num_pins;
                (!prev_pins.contains(&next_pin)).then(|| (next_pin, next_pos))
            })
            .map(|(current_pin, next_pos)| {
                #[allow(clippy::cast_sign_loss)] // distance is positive
                #[allow(clippy::cast_possible_truncation)]
                // truncation is desired
                let dist = EuclideanNorm.metric_distance(prev_pos, next_pos) as usize;
                let x_line = Array::linspace(prev_pos[0], next_pos[0], dist);
                let y_line = Array::linspace(prev_pos[1], next_pos[1], dist);
                #[allow(clippy::cast_sign_loss)] // coordinates are positive
                #[allow(clippy::cast_possible_truncation)]
                // truncation is desired
                let line_sum = x_line
                    .iter()
                    .zip(y_line.iter())
                    .map(|(&x, &y)| {
                        let pixel_idx = pos_to_pixel_idx(x, y, &img);
                        u32::from(
                            // XXX: To change to `get_pixel_unchecked` once image v0.24 lands
                            unsafe { img.get_unchecked(pixel_idx) }[0],
                        )
                    })
                    .sum();
                Line::new(current_pin, x_line, y_line, line_sum)
            })
            .max()
            .unwrap();

        prev_pins = [prev_pins[1], best.dest_pin];

        threads.push((best.xs.clone(), best.ys.clone(), best.dest_pin));
        #[allow(clippy::cast_sign_loss)] // coordinates are positive
        #[allow(clippy::cast_possible_truncation)] // truncation is desired
        best.xs.into_iter().zip(best.ys).for_each(|(x, y)| {
            let pixel_idx = pos_to_pixel_idx(x, y, &img);
            let pixel = unsafe { img.get_unchecked_mut(pixel_idx) };
            pixel[0] = 0
        });

        if best.dest_pin == prev_pin {
            break;
        }
    }
    threads
}

fn pos_to_pixel_idx<I: GenericImageView>(x: f64, y: f64, img: &I) -> RangeInclusive<usize> {
    let min_idx = y.floor() as usize * img.width() as usize + x.floor() as usize;
    min_idx..=min_idx
}

#[derive(Clone)]
struct Line {
    dest_pin: usize,
    xs: Array1<f64>,
    ys: Array1<f64>,
    sum: u32,
}

impl Line {
    pub fn new(dest_pin: usize, xs: Array1<f64>, ys: Array1<f64>, sum: u32) -> Self {
        Self {
            dest_pin,
            xs,
            ys,
            sum,
        }
    }
}

impl Eq for Line {}

impl PartialEq<Line> for Line {
    fn eq(&self, other: &Line) -> bool {
        self.sum.eq(&other.sum)
    }
}

impl Ord for Line {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.sum.cmp(&other.sum)
    }
}

impl PartialOrd<Line> for Line {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        self.sum.partial_cmp(&other.sum)
    }
}

fn preprocess(mut img: DynamicImage, radius: u32, length: u32) -> GrayImage {
    let cropped = square_crop(&mut img);
    let gray = grayscale(&cropped);
    let mut resized = resize(&gray, length, length, FilterType::Nearest);
    invert(&mut resized);
    resized.enumerate_pixels_mut().for_each(|(x, y, pixel)| {
        if (x.saturating_sub(length / 2)).pow(2) + (y.saturating_sub(length / 2)).pow(2)
            > radius.pow(2)
        {
            pixel[0] = 0;
        }
    });
    resized
}

fn square_crop<I: GenericImageView>(img: &mut I) -> SubImage<&mut I> {
    let (width, height) = img.dimensions();
    let min_edge = cmp::min(width, height);
    let top_edge = (height - min_edge) / 2;
    let left_edge = (width - min_edge) / 2;
    crop(img, left_edge, top_edge, min_edge, min_edge)
}

fn write_img(
    outfile: &mut PathBuf,
    prefix: &str,
    thread_coords: &[(Array1<f64>, Array1<f64>, usize)],
    length: u32,
) {
    let mut img_threaded = GrayImage::from_pixel(length, length, Luma([255]));
    for (x_line, y_line, _) in thread_coords {
        #[allow(clippy::cast_sign_loss)] // coordinates are positive
        #[allow(clippy::cast_possible_truncation)] // truncation is desired
        x_line.into_iter().zip(y_line).for_each(|(&x, &y)| {
            let pixel_idx = pos_to_pixel_idx(x, y, &img_threaded);
            let pixel = unsafe { img_threaded.get_unchecked_mut(pixel_idx) };
            pixel[0] = 0;
        });
    }
    outfile.set_file_name(format!("{}_threaded", prefix));
    outfile.set_extension("png");
    img_threaded
        .save(&outfile)
        .expect("Failed to save threaded image");
}

fn write_csv(
    out_dir: &mut PathBuf,
    prefix: &str,
    thread_coords: &[(Array1<f64>, Array1<f64>, usize)],
    opt: &Opt,
) {
    out_dir.set_file_name(format!("{}_threads", prefix));
    out_dir.set_extension("csv");

    let optional_header = opt.header.then(|| {
        if opt.write_coords {
            vec!["x1", "y1", "x2", "y2"]
        } else {
            vec!["pins"]
        }
    });

    let mut writer = Writer::from_path(&out_dir).expect("Failed to save threads CSV");

    if let Some(header) = optional_header {
        writer
            .write_record(&header)
            .expect("Failed to write header");
    }
    let formatter = if opt.write_coords {
        |(x_line, y_line, _): &(Array1<f64>, Array1<f64>, usize)| {
            let last = x_line.len() - 1;
            vec![
                format!("{}", x_line[0] as u32),
                format!("{}", y_line[0] as u32),
                format!("{}", x_line[last] as u32),
                format!("{}", y_line[last] as u32),
            ]
        }
    } else {
        |(_, _, pin): &(Array1<f64>, Array1<f64>, usize)| vec![format!("{}", pin)]
    };
    for thread_coord in thread_coords {
        writer
            .write_record(formatter(thread_coord))
            .expect("Failed to write thread");
    }
}
#[cfg(test)]
mod tests {
    use clap::IntoApp;

    use super::*;

    #[test]
    fn verify_app() {
        Opt::into_app().debug_assert();
    }
}
