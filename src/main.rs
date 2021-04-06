use image::{
    imageops::crop, imageops::grayscale, imageops::invert, imageops::resize, imageops::FilterType,
    open, DynamicImage, GenericImageView, GrayImage, ImageBuffer, Luma, SubImage,
};
use ndarray::Array;
use std::{cmp, path::PathBuf};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "pipetone",
    about = "Rust port of threadTone.py (https://github.com/theveloped/ThreadTone)
Generates a halftone representation of an image made of thread."
)]
struct Opt {
    #[structopt(parse(from_os_str), help = "input image path")]
    path: PathBuf,
    #[structopt(short, long, default_value = "1000")]
    threads: usize,
    #[structopt(short, long, default_value = "200")]
    pins: usize,
    #[structopt(short, long)]
    radius: Option<u32>,
    #[structopt(short, long, parse(from_os_str), help = "output image path")]
    output: Option<PathBuf>,
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
        threads,
        pins,
        radius,
        output,
    } = Opt::from_args();
    let out_path = match output {
        Some(out_path) => out_path,
        None => {
            let mut output = path
                .parent()
                .expect("Couldn't find directory to save threaded image")
                .to_path_buf();
            output.push("threaded.png");
            output
        }
    };
    let img = open(path).expect("Couldn't load target image");
    let img_threaded = thread_img(img, threads, pins, radius);
    img_threaded
        .save(out_path)
        .expect("Failed to save threaded image");
}

fn thread_img(
    img: DynamicImage,
    max_threads: usize,
    num_pins: usize,
    radius: Option<u32>,
) -> GrayImage {
    let (mut img_preprocessed, radius, length) = preprocess_img(img, radius);
    let pin_coords = Array::linspace(0., 2. * std::f32::consts::PI, num_pins + 1)
        .iter()
        .map(|alpha| {
            (
                (radius as f32 * (1. + alpha.cos())),
                (radius as f32 * (1. + alpha.sin())),
            )
        })
        .collect::<Vec<_>>();
    let mut img_result = ImageBuffer::from_fn(length, length, |_, _| Luma([255u8]));
    let mut lines = Vec::with_capacity(max_threads);
    let mut prev_pins = Vec::with_capacity(2);
    let mut old_pin = 0;
    let mut best_pin = 0;
    let mut best_xline = Array::default(1);
    let mut best_yline = Array::default(1);
    for i in 0..max_threads {
        let mut best_line = 0;
        let old_coord = pin_coords[old_pin];
        for i in 1..num_pins {
            let pin = (old_pin + i) % num_pins;
            let pin_coord = pin_coords[pin];
            let length = euclidean(old_coord, pin_coord) as usize;
            let x_line =
                Array::linspace(old_coord.0 as f32, pin_coord.0 as f32, length).mapv(|x| x as u32);
            let y_line =
                Array::linspace(old_coord.1 as f32, pin_coord.1 as f32, length).mapv(|y| y as u32);
            let line_sum = x_line
                .into_iter()
                .zip(y_line.into_iter())
                .map(|(x, y)| img_preprocessed[(*x, *y)].0[0] as u32)
                .sum();
            if line_sum > best_line && !prev_pins.contains(&pin) {
                best_line = line_sum;
                best_xline = x_line;
                best_yline = y_line;
                best_pin = pin;
            }
        }

        if prev_pins.len() > 2 {
            prev_pins.pop();
        }
        prev_pins.push(best_pin);

        best_xline
            .into_iter()
            .zip(best_yline.into_iter())
            .for_each(|(&x, &y)| {
                img_result[(x, y)].0[0] = 0;
                img_preprocessed[(x, y)].0[0] = 0;
            });

        lines.push((old_pin, best_pin));

        if best_pin == old_pin {
            break;
        }

        old_pin = best_pin;

        print!("\rPlacing thread {}/{}", i + 1, max_threads);
    }
    println!("");
    img_result
}

fn preprocess_img(mut img: DynamicImage, radius: Option<u32>) -> (GrayImage, u32, u32) {
    let img_cropped = square_crop(&mut img);
    let img_gray = grayscale(&img_cropped);
    let radius = radius.unwrap_or(img_gray.height());
    let length = 2 * radius + 1;
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
    (img_resized, radius, length)
}

fn square_crop<I: GenericImageView>(img: &mut I) -> SubImage<&mut I> {
    let (width, height) = img.dimensions();
    let min_edge = cmp::min(width, height);
    let top_edge = (height - min_edge) / 2;
    let left_edge = (width - min_edge) / 2;
    crop(img, left_edge, top_edge, min_edge, min_edge)
}

fn euclidean(u: (f32, f32), v: (f32, f32)) -> f32 {
    ((u.0 - v.0).powi(2) + (u.1 - v.1).powi(2)).sqrt()
}
