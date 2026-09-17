use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;

#[repr(C)]
struct IfReq {
    ifr_name: [u8; 16],
    ifr_flags: u16,
    _pad: [u8; 22],
}

const IFF_TAP: u16 = 0x0002;
const IFF_NO_PI: u16 = 0x1000;
const TUNSETIFF: libc::c_ulong = 0x400454ca;


const OUR_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
const OUR_IP: [u8; 4] = [192, 168, 99, 2];


fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for chunk in data.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]]) as u32
        } else {
            (chunk[0] as u32) << 8
        };
        sum += word;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn main() -> anyhow::Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/net/tun")?;

    let mut req = IfReq {
        ifr_name: [0; 16],
        ifr_flags: IFF_TAP | IFF_NO_PI,
        _pad: [0; 22],
    };
    req.ifr_name[..4].copy_from_slice(b"tap0");

    let ret = unsafe { libc::ioctl(file.as_raw_fd(), TUNSETIFF, &mut req) };
    if ret < 0 {
        return Err(anyhow::anyhow!(
            "ioctl(TUNSETIFF) failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    println!("⚡ tap0 opened. Our MAC: {:02x?}, Our IP: {}.{}.{}.{}",
        OUR_MAC, OUR_IP[0], OUR_IP[1], OUR_IP[2], OUR_IP[3]);
    println!("Waiting for packets. Run: ping 192.168.99.2\n");

    let mut buf = [0u8; 2048];
    let mut file = file;

    loop {
        let n = file.read(&mut buf)?;
        if n < 14 { continue; }

        let ethertype = u16::from_be_bytes([buf[12], buf[13]]);

        match ethertype {
            0x0806 => handle_arp(&buf[..n], &mut file),
            0x0800 => handle_ipv4(&buf[..n], &mut file),
            _ => {} 
        }
    }
}


fn handle_arp(frame: &[u8], file: &mut std::fs::File) {
    if frame.len() < 42 { return; }

    let htype = u16::from_be_bytes([frame[14], frame[15]]);
    let ptype = u16::from_be_bytes([frame[16], frame[17]]);
    let oper  = u16::from_be_bytes([frame[20], frame[21]]);
    let target_ip = &frame[38..42];

    if htype != 1 || ptype != 0x0800 || oper != 1 || target_ip != OUR_IP {
        return;
    }

    let sender_mac = &frame[22..28];
    let sender_ip  = &frame[28..32];

    println!("📥 ARP request: who has {}.{}.{}.{}? (from {:02x?})",
        target_ip[0], target_ip[1], target_ip[2], target_ip[3], sender_mac);

    let mut reply = [0u8; 42];


    reply[0..6].copy_from_slice(sender_mac);   
    reply[6..12].copy_from_slice(&OUR_MAC);   
    reply[12..14].copy_from_slice(&0x0806u16.to_be_bytes());

    
    reply[14..16].copy_from_slice(&1u16.to_be_bytes());     
    reply[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
    reply[18] = 6;  
    reply[19] = 4;  
    reply[20..22].copy_from_slice(&2u16.to_be_bytes());      
    reply[22..28].copy_from_slice(&OUR_MAC);   
    reply[28..32].copy_from_slice(&OUR_IP);    
    reply[32..38].copy_from_slice(sender_mac); 
    reply[38..42].copy_from_slice(sender_ip);  

    file.write_all(&reply).unwrap();
    println!("📤 ARP reply sent\n");
}

fn handle_ipv4(frame: &[u8], file: &mut std::fs::File) {
    if frame.len() < 34 { return; } 

    let ihl = (frame[14] & 0x0F) as usize * 4;
    let total_len = u16::from_be_bytes([frame[16], frame[17]]) as usize;
    let protocol = frame[14 + 9];
    let src_ip = &frame[14 + 12..14 + 16];
    let dst_ip = &frame[14 + 16..14 + 20];

    if dst_ip != OUR_IP { return; }

    let ip_hdr = &frame[14..14 + ihl];
    if internet_checksum(ip_hdr) != 0 {
        println!("⚠️  Bad IP checksum, dropping");
        return;
    }

    if protocol == 1 {
        let icmp_start = 14 + ihl;
        let icmp_end = 14 + total_len;
        if frame.len() < icmp_end || icmp_end - icmp_start < 8 { return; }

        let icmp_type = frame[icmp_start];
        let icmp_code = frame[icmp_start + 1];

        if icmp_type == 8 && icmp_code == 0 {
            let icmp_len = icmp_end - icmp_start;
            let reply_len = 14 + 20 + icmp_len;
            let mut reply = vec![0u8; reply_len];

            reply[0..6].copy_from_slice(&frame[6..12]);  
            reply[6..12].copy_from_slice(&OUR_MAC);       
            reply[12..14].copy_from_slice(&0x0800u16.to_be_bytes());


            reply[14] = 0x45; 
            reply[15] = 0x00; 
            let ip_total = (20 + icmp_len) as u16;
            reply[16..18].copy_from_slice(&ip_total.to_be_bytes());
            reply[18..20].copy_from_slice(&0u16.to_be_bytes()); 
            reply[20..22].copy_from_slice(&0x4000u16.to_be_bytes()); 
            reply[22] = 64;  
            reply[23] = 1;    
            reply[24..26].copy_from_slice(&0u16.to_be_bytes()); 
            reply[26..30].copy_from_slice(&OUR_IP); 
            reply[30..34].copy_from_slice(src_ip);   

            
            let ip_cksum = internet_checksum(&reply[14..34]);
            reply[24..26].copy_from_slice(&ip_cksum.to_be_bytes());

            
           
            reply[34..].copy_from_slice(&frame[icmp_start..icmp_end]);
            reply[34] = 0;
            reply[36..38].copy_from_slice(&0u16.to_be_bytes());
            let icmp_cksum = internet_checksum(&reply[34..]);
            reply[36..38].copy_from_slice(&icmp_cksum.to_be_bytes());

            file.write_all(&reply).unwrap();

            let seq = u16::from_be_bytes([frame[icmp_start + 6], frame[icmp_start + 7]]);
            println!("📤 ICMP echo reply sent (seq={}, {} bytes)", seq, icmp_len);
        }
    }
}
