use csv::Writer;
use image::{
    imageops::crop, imageops::grayscale, imageops::invert, imageops::resize, imageops::FilterType,
    open, DynamicImage, GenericImageView, GrayImage, Luma, SubImage,
};
use ndarray::{array, Array, Array1};
use std::{cmp, f32::consts::PI, path::Path, path::PathBuf};
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
        help = "Side length of output image. Should be no greater than l = min(width, height). Defaults to l otherwise, or if omitted"
    )]
    radius: Option<u32>,
    #[structopt(short, long, parse(from_os_str), help = "Path to output image")]
    output: Option<PathBuf>,
    #[structopt(
        long,
        help = "Save thread start and end coordinates as CSV. Unset by default"
    )]
    csv: bool,
    #[structopt(
        long,
        help = "Include CSV header line: \"x1, y1, x2, y2\". Unset by default. Used with --csv"
    )]
    csv_header: bool,
    #[structopt(
        long,
        help = "Skip image generation. Unset by default. Used with --csv"
    )]
    no_img: bool,
    #[structopt(
        long,
        help = "Write thread end-point coordinates instead of pin numbers to CSV. Used with --csv"
    )]
    write_threads: bool,
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
        csv_header,
        no_img,
        write_threads,
    } = Opt::from_args();

    let img = open(&path).expect("Couldn't load target image");
    let min_edge = cmp::min(img.width(), img.height());
    let radius = cmp::min(min_edge, radius.unwrap_or(min_edge));
    let length = radius * 2 + 1;
    let thread_coords = generate_threads(img, threads, pins, radius, length);

    let mut out_dir = output.unwrap_or(path.parent().unwrap_or(Path::new(".")).to_path_buf());
    let prefix = format!(
        "{}_{}_{}",
        path.file_stem().unwrap().to_str().unwrap(),
        pins,
        threads
    );

    if !no_img {
        save_img(&mut out_dir, &prefix, &thread_coords, length);
    }

    if csv {
        let optional_header = csv_header.then(|| {
            if write_threads {
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
            write_threads,
        );
    }
}

fn save_img(
    out_dir: &mut PathBuf,
    prefix: &str,
    thread_coords: &[(Array1<u32>, Array1<u32>, usize)],
    length: u32,
) {
    let mut img_threaded = GrayImage::from_pixel(length, length, Luma([255]));
    thread_coords.iter().for_each(|(x_line, y_line, _)| {
        x_line.iter().zip(y_line.iter()).for_each(|(x, y)| {
            img_threaded[(*x, *y)].0[0] = 0;
        })
    });
    out_dir.set_file_name(format!("{}_threaded", prefix));
    out_dir.set_extension("png");
    img_threaded
        .save(&out_dir)
        .expect("Failed to save threaded image");
}

fn save_csv(
    out_dir: &mut PathBuf,
    prefix: &str,
    thread_coords: &[(Array1<u32>, Array1<u32>, usize)],
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
        |(x_line, y_line, _): &(Array1<u32>, Array1<u32>, usize)| {
            let last = x_line.len() - 1;
            vec![
                format!("{}", x_line[0]),
                format!("{}", y_line[0]),
                format!("{}", x_line[last]),
                format!("{}", y_line[last]),
            ]
        }
    } else {
        |(_, _, pin): &(Array1<u32>, Array1<u32>, usize)| vec![format!("{}", pin)]
    };
    thread_coords.iter().map(formatter).for_each(|thread| {
        writer.write_record(thread).expect("Failed to write thread");
    });
}

fn generate_threads(
    img: DynamicImage,
    max_threads: usize,
    num_pins: usize,
    radius: u32,
    length: u32,
) -> Vec<(Array1<u32>, Array1<u32>, usize)> {
    let mut img_preprocessed = preprocess_img(img, radius, length);
    let pin_coords = Array::linspace(0., 2. * PI, num_pins + 1).mapv(|alpha| {
        array![
            radius as f32 * (1. + alpha.cos()),
            radius as f32 * (1. + alpha.sin()),
        ]
    });

    let mut threads = Vec::with_capacity(max_threads);
    let mut prev_pins = [0; 2];
    let mut old_pin = 0;
    let mut best_pin = 0;
    let mut best_xline = array![];
    let mut best_yline = array![];

    for i in 0..max_threads {
        let mut best_line = 0;
        let old_coord = &pin_coords[old_pin];
        for i in 1..num_pins {
            let pin = (old_pin + i) % num_pins;
            let pin_coord = &pin_coords[pin];
            let length = euclidean(&old_coord, &pin_coord) as usize;
            let x_line = Array::linspace(old_coord[0], pin_coord[0], length).mapv(|x| x as u32);
            let y_line = Array::linspace(old_coord[1], pin_coord[1], length).mapv(|y| y as u32);
            let line_sum = x_line
                .iter()
                .zip(y_line.iter())
                .map(|(&x, &y)| img_preprocessed[(x, y)].0[0] as u32)
                .sum();
            if line_sum > best_line && !prev_pins.contains(&pin) {
                best_line = line_sum;
                best_xline = x_line;
                best_yline = y_line;
                best_pin = pin;
            }
        }

        prev_pins.swap(0, 1);
        prev_pins[1] = best_pin;

        threads.push((best_xline.clone(), best_yline.clone(), best_pin));
        best_xline
            .iter()
            .zip(best_yline.iter())
            .for_each(|(&x, &y)| {
                img_preprocessed[(x, y)].0[0] = 0;
            });

        if best_pin == old_pin {
            break;
        }

        old_pin = best_pin;

        print!("\rPlacing thread {}/{}", i + 1, max_threads);
    }
    println!("");
    threads
}

fn euclidean(old_coord: &Array1<f32>, pin_coord: &Array1<f32>) -> f32 {
    let mut diff = old_coord - pin_coord;
    diff.mapv_inplace(|axis| axis.powf(2.));
    diff.sum().sqrt()
}

fn preprocess_img(mut img: DynamicImage, radius: u32, length: u32) -> GrayImage {
    let img_cropped = square_crop(&mut img);
    let img_gray = grayscale(&img_cropped);
    let mut img_resized = resize(&img_gray, length, length, FilterType::Nearest);
    invert(&mut img_resized);
    img_resized
        .enumerate_pixels_mut()
        .for_each(|(x, y, pixel)| {
            if (x as i32 - length as i32 / 2).pow(2) + (y as i32 - length as i32 / 2).pow(2)
                > radius.pow(2) as i32
            {
                pixel.0[0] = 0;
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
