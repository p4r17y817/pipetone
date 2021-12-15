use csv::Writer;
use image::{
    imageops::crop, imageops::grayscale, imageops::invert, imageops::resize, imageops::FilterType,
    open, DynamicImage, GenericImageView, GrayImage, Luma, SubImage,
};
use nalgebra::{vector, EuclideanNorm, Norm};
use ndarray::{Array, Array1};
use rayon::prelude::*;
use std::{cmp, f32::consts::PI, path::PathBuf};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "pipetone 🧵",
    about = "🚀 A fast, Rusty port of threadTone.py (https://github.com/theveloped/ThreadTone)
Generates a circular, halftone representation of an image using \"threads\"."
)]
struct Opt {
    #[structopt(parse(from_os_str), help = "Path to target image")]
    path: PathBuf,
    #[structopt(short, long, default_value = "500", help = "# pins on loom")]
    pins: usize,
    #[structopt(short, long, default_value = "1000", help = "MAX # threads")]
    threads: usize,
    #[structopt(
        short,
        long,
        help = "Radius of output image in pixels. Should be no greater than l = min(input_width, input_height). Defaults to l otherwise, or if omitted"
    )]
    radius: Option<u32>,
    #[structopt(
        short,
        long,
        parse(from_os_str),
        help = "Path to output image. Defaults to input's directory"
    )]
    output: Option<PathBuf>,
    #[structopt(
        long,
        help = "Save thread start and end coordinates as CSV. Unset by default"
    )]
    csv: bool,
    #[structopt(
        long,
        help = "Include CSV header line. If --write-coords then the value is \"x1, y1, x2, y2\", otherwise \"pins\". Unset by default. Used with --csv"
    )]
    header: bool,
    #[structopt(
        long,
        help = "Skip image generation. Unset by default. Used with --csv"
    )]
    no_img: bool,
    #[structopt(
        long,
        help = "Write thread end-point pixel coordinates instead of pin numbers to CSV. Used with --csv"
    )]
    write_coords: bool,
}

fn main() {
    println!(
        "           _            __                 
    ____  (_)___  ___  / /_____  ____  ___ 
   / __ \\/ / __ \\/ _ \\/ __/ __ \\/ __ \\/ _ \\
  / /_/ / / /_/ /  __/ /_/ /_/ / / / /  __/
 / .___/_/ .___/\\___/\\__/\\____/_/ /_/\\___/ 
/_/     /_/
"
    );

    let Opt {
        path,
        pins,
        threads,
        radius,
        output,
        csv,
        header,
        no_img,
        write_coords,
    } = Opt::from_args();

    let img = open(&path).expect("Couldn't load target image");
    let min_edge_length = img.width().min(img.height());
    let radius = radius.map_or(min_edge_length, |radius| radius.min(min_edge_length));
    let length = radius * 2 + 1;

    let thread_coords = generate_threads(img, threads, pins, radius, length);

    let prefix = format!(
        "{}_{}_{}",
        path.file_stem().unwrap().to_str().unwrap(),
        pins,
        threads
    );
    let mut out_dir = output.unwrap_or(path);

    if !no_img {
        save_img(&mut out_dir, &prefix, &thread_coords, length);
    }

    if csv {
        let optional_header = header.then(|| {
            if write_coords {
                vec!["x1", "y1", "x2", "y2"]
            } else {
                vec!["pins"]
            }
        });
        save_csv(
            &mut out_dir,
            &prefix,
            &thread_coords,
            optional_header,
            write_coords,
        );
    }
}

fn save_img(
    out_dir: &mut PathBuf,
    prefix: &str,
    thread_coords: &[(Array1<f64>, Array1<f64>, usize)],
    length: u32,
) {
    let mut img_threaded = GrayImage::from_pixel(length, length, Luma([255]));
    for (x_line, y_line, _) in thread_coords {
        #[allow(clippy::cast_sign_loss)] // coordinates are positive
        #[allow(clippy::cast_possible_truncation)] // truncation is desired
        x_line.iter().zip(y_line.iter()).for_each(|(x, y)| {
            img_threaded[(*x as u32, *y as u32)].0[0] = 0;
        });
    }
    out_dir.set_file_name(format!("{}_threaded", prefix));
    out_dir.set_extension("png");
    img_threaded
        .save(&out_dir)
        .expect("Failed to save threaded image");
}

fn save_csv(
    out_dir: &mut PathBuf,
    prefix: &str,
    thread_coords: &[(Array1<f64>, Array1<f64>, usize)],
    optional_header: Option<Vec<&str>>,
    write_threads: bool,
) {
    out_dir.set_file_name(format!("{}_threads", prefix));
    out_dir.set_extension("csv");
    let mut writer = Writer::from_path(&out_dir).expect("Failed to save threads CSV");
    if let Some(header) = optional_header {
        writer
            .write_record(&header)
            .expect("Failed to write header");
    }
    let formatter = if write_threads {
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

fn generate_threads(
    img: DynamicImage,
    max_threads: usize,
    num_pins: usize,
    radius: u32,
    length: u32,
) -> Vec<(Array1<f64>, Array1<f64>, usize)> {
    let mut img_preprocessed = preprocess_img(img, radius, length);

    let pin_positions = Array::linspace(0., 2. * PI, num_pins + 1)
        .to_vec()
        .into_iter()
        .map(|alpha| {
            vector![
                f64::from(radius) * f64::from(1. + alpha.cos()),
                f64::from(radius) * f64::from(1. + alpha.sin())
            ]
        })
        .collect::<Vec<_>>();

    let mut threads = Vec::with_capacity(max_threads);
    let mut prev_pins = [0; 2];

    for i in 0..max_threads {
        let prev_pin = prev_pins[1];
        let prev_pos = &pin_positions[prev_pin];
        let lines = pin_positions[prev_pin..]
            .par_iter()
            .chain(&pin_positions[..prev_pin])
            .map(|next_pos| {
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
                        u32::from(img_preprocessed[(x.floor() as u32, y.floor() as u32)][0])
                    })
                    .sum();
                (x_line, y_line, line_sum)
            });
        let best = (1..num_pins)
            .into_par_iter()
            .map(|j| (prev_pin + j) % num_pins)
            .zip(lines)
            .filter(|(next_pin, _)| !prev_pins.contains(&next_pin))
            .map(|(next_pin, (x_line, y_line, line_sum))| {
                Line::new(next_pin, x_line, y_line, line_sum)
            })
            .max()
            .unwrap();

        prev_pins = [prev_pins[1], best.dest_pin];

        threads.push((best.xs.clone(), best.ys.clone(), best.dest_pin));
        #[allow(clippy::cast_sign_loss)] // coordinates are positive
        #[allow(clippy::cast_possible_truncation)] // truncation is desired
        best.xs.iter().zip(best.ys.iter()).for_each(|(&x, &y)| {
            img_preprocessed[(x as u32, y as u32)][0] = 0;
        });

        if best.dest_pin == prev_pin {
            break;
        }

        print!("\rPlacing thread {}/{}", i + 1, max_threads);
    }
    println!();
    threads
}

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

fn preprocess_img(mut img: DynamicImage, radius: u32, length: u32) -> GrayImage {
    let img_cropped = square_crop(&mut img);
    let img_gray = grayscale(&img_cropped);
    let mut img_resized = resize(&img_gray, length, length, FilterType::Nearest);
    invert(&mut img_resized);
    img_resized
        .enumerate_pixels_mut()
        .for_each(|(x, y, pixel)| {
            if (x.saturating_sub(length / 2)).pow(2) + (y.saturating_sub(length / 2)).pow(2)
                > radius.pow(2)
            {
                pixel[0] = 0;
            }
        });
    img_resized
}

fn square_crop<I: GenericImageView>(img: &mut I) -> SubImage<&mut I> {
    let (width, height) = img.dimensions();
    let min_edge = cmp::min(width, height);
    let top_edge = (height - min_edge) / 2;
    let left_edge = (width - min_edge) / 2;
    crop(img, left_edge, top_edge, min_edge, min_edge)
}
