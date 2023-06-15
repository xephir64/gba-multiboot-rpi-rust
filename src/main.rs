use std::env;
use std::thread;
use std::process;
use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;

use rppal::spi::{Bus, Mode, SlaveSelect, Spi};

/*fn spi_8_rw(spi: &mut Spi, arr: &mut[u8; 4]) {
    let mut buf_read = [0u8; 4];
    spi.transfer(&mut buf_read, &mut *arr).unwrap();
    *arr = buf_read;
}*/

fn spi_32_rw(spi: &mut Spi, val: u32) -> u32 {
    let mut buf_write = [(val >> 24) as u8, (val >> 16) as u8, (val >> 8) as u8, val as u8];
    let mut buf_read = [0u8; 4];
    spi.transfer(&mut buf_read, &mut buf_write).unwrap();
    let response = ((buf_read[0] as u32) << 24) | ((buf_read[1] as u32) << 16) | ((buf_read[2] as u32) << 8) | buf_read[3] as u32;
    return response;
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: cargo run mb_file");
        process::exit(1);
    }

    let path = &args[1];

    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            eprintln!("Open Error");
            process::exit(1);
        }
    };

    let fsize = metadata.len() as u32;

    if fsize > 0x40000 {
        eprintln!("Max file size is 256KB");
        process::exit(1);
    }

    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => {
            eprintln!("Fopen Error");
            process::exit(1);
        }
    };

    let mut buf_reader = BufReader::new(file);
    let mut fdata = Vec::with_capacity((fsize + 0x10) as usize);
    buf_reader
        .read_to_end(&mut fdata)
        .expect("Error reading file");
    drop(buf_reader);

    // Waiting for GBA
    println!("Waiting for GBA...");
    let mut recv: u32;

    let mut spi = Spi::new(Bus::Spi0, SlaveSelect::Ss1, 1_000_000, Mode::Mode3)
        .expect("Failed to initialize SPI");

    loop {
        recv = spi_32_rw(&mut spi, 0x6202) >> 16;
        thread::sleep(std::time::Duration::from_millis(10));

        if recv == 0x7202 {
            break;
        }
    }

    // Sending header
    println!("Sending header.");
    spi_32_rw(&mut spi, 0x6102);

    let fdata16: &[u16] = unsafe { std::mem::transmute(&fdata[..]) };
    for i in (0..0xC0).step_by(2) {
        spi_32_rw(&mut spi, fdata16[i as usize / 2] as u32);
    }

    spi_32_rw(&mut spi, 0x6200);

    // Getting encryption and crc seeds
    println!("Getting encryption and crc seeds.");
    spi_32_rw(&mut spi, 0x6202);
    spi_32_rw(&mut spi, 0x63D1);

    let token = spi_32_rw(&mut spi, 0x63D1);
    let crc_a = (token >> 16) & 0xFF;
    let mut seed = 0xFFFF00D1 | (crc_a << 8);
    let crc_a = (crc_a + 0xF) & 0xFF;

    spi_32_rw(&mut spi, 0x6400 | crc_a);

    let mut fsize = fsize + 0xF;
    fsize &= !0xF;

    let token = spi_32_rw(&mut spi, (fsize - 0x190) / 4);
    let crc_b = (token >> 16) & 0xFF;
    let mut crc_c = 0xC387;

    // Sending
    println!("Sending...");
    let fdata32: &[u32] = unsafe { std::mem::transmute(&fdata[..]) };

    for i in (0xC0..fsize).step_by(4) {
        let dat = fdata32[i as usize / 4];

        // CRC
        let mut tmp = dat;

        for _ in 0..32 {
            let bit = (crc_c ^ tmp) & 1;
            crc_c = (crc_c >> 1) ^ (if bit != 0 { 0xc37b } else { 0 });
            tmp >>= 1;
        }

        // Encrypt
        seed = seed.wrapping_mul(0x6F646573) + 1;
        let dat = seed ^ dat ^ (0xFE000000 - i) ^ 0x43202F2F;

        // Send
        let chk = spi_32_rw(&mut spi, dat) >> 16;

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
    spi_32_rw(&mut spi, 0x0065);

    loop {
        recv = spi_32_rw(&mut spi, 0x0065) >> 16;
        thread::sleep(std::time::Duration::from_millis(10));

        if recv == 0x0075 {
            break;
        }
    }
    spi_32_rw(&mut spi, 0x0066);
    let crc_gba = spi_32_rw(&mut spi, crc_c & 0xFFFF) >> 16;
    println!("Gba: {:x}, Cal: {:x}", crc_gba, crc_c);
}
