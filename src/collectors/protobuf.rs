//! Decodificador mínimo de protobuf "sin esquema" (wire format).
//! Antigravity guarda sus metadatos como blobs protobuf sin publicar el .proto,
//! así que leemos los campos por número.

#[derive(Debug, Clone, Copy)]
pub enum Val<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed64,
    Fixed32,
}

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut r = 0u64;
    let mut shift = 0;
    loop {
        let x = *b.get(*i)?;
        *i += 1;
        if shift < 64 {
            r |= ((x & 0x7f) as u64) << shift;
        }
        if x < 0x80 {
            return Some(r);
        }
        shift += 7;
        if shift > 70 {
            return None;
        }
    }
}

/// Devuelve los campos de primer nivel de un mensaje, o None si no es protobuf válido.
pub fn fields(b: &[u8]) -> Option<Vec<(u32, Val<'_>)>> {
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let key = varint(b, &mut i)?;
        let (field, wire) = ((key >> 3) as u32, key & 7);
        if field == 0 {
            return None;
        }
        let v = match wire {
            0 => Val::Varint(varint(b, &mut i)?),
            1 => {
                i += 8;
                Val::Fixed64
            }
            5 => {
                i += 4;
                Val::Fixed32
            }
            2 => {
                let len = varint(b, &mut i)? as usize;
                let end = i.checked_add(len)?;
                let s = b.get(i..end)?;
                i = end;
                Val::Bytes(s)
            }
            _ => return None,
        };
        if i > b.len() {
            return None;
        }
        out.push((field, v));
    }
    Some(out)
}

/// Primer submensaje/bytes con ese número de campo.
pub fn bytes<'a>(f: &[(u32, Val<'a>)], n: u32) -> Option<&'a [u8]> {
    f.iter().find_map(|(k, v)| match v {
        Val::Bytes(b) if *k == n => Some(*b),
        _ => None,
    })
}

pub fn all_bytes<'a>(f: &[(u32, Val<'a>)], n: u32) -> Vec<&'a [u8]> {
    f.iter()
        .filter_map(|(k, v)| match v {
            Val::Bytes(b) if *k == n => Some(*b),
            _ => None,
        })
        .collect()
}

pub fn uint(f: &[(u32, Val<'_>)], n: u32) -> u64 {
    f.iter()
        .find_map(|(k, v)| match v {
            Val::Varint(x) if *k == n => Some(*x),
            _ => None,
        })
        .unwrap_or(0)
}

pub fn string(f: &[(u32, Val<'_>)], n: u32) -> Option<String> {
    bytes(f, n).and_then(|b| std::str::from_utf8(b).ok()).map(str::to_string)
}

/// Sigue una ruta de submensajes, p. ej. `path(b, &[1, 4])` = campo 1 → campo 4.
pub fn path<'a>(b: &'a [u8], p: &[u32]) -> Option<&'a [u8]> {
    let mut cur = b;
    for n in p {
        cur = bytes(&fields(cur)?, *n)?;
    }
    Some(cur)
}
