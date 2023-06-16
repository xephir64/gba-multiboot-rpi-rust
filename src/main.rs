use std::env;
use std::thread;
use std::process;
use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;

use rppal::spi::{Bus, Mode, SlaveSelect, Spi};

fn spi_32_rw(spi: &mut Spi, val: u32) -> u32 {
    let mut buf_write = [(val >> 24) as u8, (val >> 16) as u8, (val >> 8) as u8, val as u8];
    let mut buf_read = [0u8; 4];
    spi.transfer(&mut buf_read, &mut buf_write).unwrap();
    ((buf_read[0] as u32) << 24) | ((buf_read[1] as u32) << 16) | ((buf_read[2] as u32) << 8) | buf_read[3] as u32
}

fn upload_mb(spi: &mut Spi, mb_file: File, length: u32) {
    let mut buf_reader = BufReader::new(mb_file);
    let mut data = Vec::with_capacity((length + 0x10) as usize);
    buf_reader.read_to_end(&mut data).expect("Error reading file");

    // Waiting for GBA
    println!("Waiting for GBA...");
    let mut recv: u32;

    loop {
        recv = spi_32_rw(spi, 0x6202) >> 16;
        thread::sleep(std::time::Duration::from_millis(10));

        if recv == 0x7202 {
            break;
        }
    }

    println!("GBA Found.");

    // Sending header of size 0xC0
    let data16: &[u16] = unsafe { std::mem::transmute(&data[..]) };
    println!("Sending header.");
    spi_32_rw(spi, 0x6102);
    for i in (0..0xC0).step_by(2) {
        spi_32_rw(spi, data16[i as usize / 2] as u32);
    }
    spi_32_rw(spi, 0x6200);

    // Getting encryption and crc seeds
    println!("Getting seeds.");
    spi_32_rw(spi, 0x6202);
    spi_32_rw(spi, 0x63D1);

    let token = spi_32_rw(spi, 0x63D1);
    let crc_a = (token >> 16) & 0xFF;
    let mut seed = 0xFFFF00D1 | (crc_a << 8);
    let crc_a = (crc_a + 0xF) & 0xFF;

    spi_32_rw(spi, 0x6400 | crc_a);

    let mut fsize = length + 0xF;
    fsize &= !0xF;

    let token = spi_32_rw(spi, (fsize - 0x190) / 4);

    if (token >> 24) != 0x73 {
        eprintln!("Failed Handshake");
        return;
    }

    let crc_b = (token >> 16) & 0xFF;
    let mut crc_c = 0xC387;

    // Sending
    println!("Sending file of size: {}", fsize);
    let data32: &[u32] = unsafe { std::mem::transmute(&data[..]) };

    for i in (0xC0..fsize).step_by(4) {
        let chunk = data32[i as usize / 4];

        // CRC
        let mut tmp = chunk;

        for _ in 0..32 {
            let bit = (crc_c ^ tmp) & 1;
            crc_c = (crc_c >> 1) ^ (if bit != 0 { 0xc37b } else { 0 });
            tmp >>= 1;
        }

        // Encrypt
        seed = seed.wrapping_mul(0x6F646573) + 1;
        let chunk = seed ^ chunk ^ (0xFE000000 - i) ^ 0x43202F2F;

        // Send
        let chk = spi_32_rw(spi, chunk) >> 16;

        if chk != (i & 0xFFFF) {
            eprintln!("Transmission error at byte {}: chk == {:08x}", i, chk);
            process::exit(1);
        }
    }
    let mut tmp = 0xFFFF0000 | (crc_b << 8) | crc_a;
    for _ in 0..32 {
        let bit = (crc_c ^ tmp) & 1;
        crc_c = (crc_c >> 1) ^ (if bit != 0 { 0xc37b } else { 0 });
        tmp >>= 1;
    }
    println!("Waiting for checksum...");
    spi_32_rw(spi, 0x0065);

    loop {
        recv = spi_32_rw(spi, 0x0065) >> 16;
        thread::sleep(std::time::Duration::from_millis(10));

        if recv == 0x0075 {
            break;
        }
    }
    spi_32_rw(spi, 0x0066);
    let crc_gba = spi_32_rw(spi, crc_c & 0xFFFF) >> 16;
    println!("Gba CRC: {:x}, Our CRC: {:x}", crc_gba, crc_c);
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: cargo run path_to_mb_file");
        process::exit(1);
    }

    let path = &args[1];

    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            eprintln!("Failed to read file metadata");
            process::exit(1);
        }
    };

    let length = metadata.len() as u32;

    if length > 0x40000 {
        eprintln!("Max file size is 256KB");
        process::exit(1);
    }

    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => {
            eprintln!("Failed to open the file");
            process::exit(1);
        }
    };

    let mut spi = Spi::new(Bus::Spi0, SlaveSelect::Ss1, 1_000_000, Mode::Mode3)
    .expect("Failed to initialize SPI");

    upload_mb(&mut spi, file, length);

}
