use criterion::{black_box, criterion_group, criterion_main, Criterion};

#[derive(Debug, PartialEq)]
enum Osc99PayloadType {
    Body,
    Title,
    Ignore,
}

fn parse_osc99_metadata_old(metadata: &str) -> (Osc99PayloadType, bool) {
    let mut payload_type = Osc99PayloadType::Body;
    let mut is_base64 = false;

    for param in metadata.split(':') {
        if let Some(value) = param.strip_prefix("p=") {
            payload_type = match value {
                "title" => Osc99PayloadType::Title,
                "body" => Osc99PayloadType::Body,
                _ => Osc99PayloadType::Ignore,
            };
        } else if param == "e=1" {
            is_base64 = true;
        }
    }

    (payload_type, is_base64)
}

fn parse_osc99_metadata_loop_find(mut metadata: &str) -> (Osc99PayloadType, bool) {
    let mut payload_type = Osc99PayloadType::Body;
    let mut is_base64 = false;

    while !metadata.is_empty() {
        let (param, rest) = match metadata.find(':') {
            Some(idx) => {
                let p = &metadata[..idx];
                let r = &metadata[idx + 1..];
                (p, r)
            }
            None => (metadata, ""),
        };
        metadata = rest;

        if let Some(value) = param.strip_prefix("p=") {
            payload_type = match value {
                "title" => Osc99PayloadType::Title,
                "body" => Osc99PayloadType::Body,
                _ => Osc99PayloadType::Ignore,
            };
        } else if param == "e=1" {
            is_base64 = true;
        }
    }

    (payload_type, is_base64)
}

fn parse_osc99_metadata_bytes(metadata: &str) -> (Osc99PayloadType, bool) {
    let mut payload_type = Osc99PayloadType::Body;
    let mut is_base64 = false;

    let bytes = metadata.as_bytes();
    let mut start = 0;

    while start < bytes.len() {
        let mut end = start;
        while end < bytes.len() && bytes[end] != b':' {
            end += 1;
        }

        let param = &bytes[start..end];
        if param.starts_with(b"p=") {
            let value = &param[2..];
            payload_type = match value {
                b"title" => Osc99PayloadType::Title,
                b"body" => Osc99PayloadType::Body,
                _ => Osc99PayloadType::Ignore,
            };
        } else if param == b"e=1" {
            is_base64 = true;
        }

        start = end + 1;
    }

    (payload_type, is_base64)
}

fn criterion_benchmark(c: &mut Criterion) {
    let metadata_short = "p=title:e=1";
    let metadata_long = "i=some-long-id-1234:p=body:e=1:a=b:c=d:x=y";

    let mut group = c.benchmark_group("parse_osc99_metadata");

    group.bench_function("old_short", |b| {
        b.iter(|| parse_osc99_metadata_old(black_box(metadata_short)))
    });
    group.bench_function("loop_find_short", |b| {
        b.iter(|| parse_osc99_metadata_loop_find(black_box(metadata_short)))
    });
    group.bench_function("bytes_short", |b| {
        b.iter(|| parse_osc99_metadata_bytes(black_box(metadata_short)))
    });

    group.bench_function("old_long", |b| {
        b.iter(|| parse_osc99_metadata_old(black_box(metadata_long)))
    });
    group.bench_function("loop_find_long", |b| {
        b.iter(|| parse_osc99_metadata_loop_find(black_box(metadata_long)))
    });
    group.bench_function("bytes_long", |b| {
        b.iter(|| parse_osc99_metadata_bytes(black_box(metadata_long)))
    });

    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
