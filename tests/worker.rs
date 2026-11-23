//! The app's PDFium worker end to end, in a process of its own (PDFium can be
//! bound only once per process, and the worker binds it): opening, whole-page
//! renders, thumbnails shrunk from them, and tiles.

use egui::vec2;
use greenline::render::{Layer, Req, Resp, Worker};
use std::time::{Duration, Instant};

fn next(w: &Worker) -> Resp {
    w.rx.recv_timeout(Duration::from_secs(20)).expect("the worker answers")
}

#[test]
fn worker_opens_renders_and_reuses_renders_for_thumbnails() {
    let dir = std::env::temp_dir().join(format!("greenline-worker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pdf = dir.join("blank.pdf");
    let w = Worker::spawn(egui::Context::default()).unwrap();

    w.send(Req::Create { path: pdf.clone(), size: vec2(1224.0, 792.0), pages: 3 });
    assert!(matches!(next(&w), Resp::Created(Ok(_))));
    w.send(Req::Open { generation: 1, path: pdf.clone(), layout: None });
    let Resp::Opened { generation: 1, pages, source: 3 } = next(&w) else { panic!("opened") };
    assert_eq!(pages, vec![vec2(1224.0, 792.0); 3]);

    // A whole-page render of a page whose thumbnail is wanted and missing: the
    // render, then its thumbnail shrunk from it.
    {
        let mut keep = w.keep.lock().unwrap();
        keep.base.insert((7, 0));
        keep.thumbs.insert(0);
        keep.thumbs_missing.insert(0);
        keep.thumbs_generation = 1;
    }
    w.send(Req::Render { generation: 1, view: 7, page: 0, scale: 1.0, origin: [0, 0], size: [1224, 792], layer: Layer::Base });
    let Resp::Rendered { layer: Layer::Base, image, .. } = next(&w) else { panic!("base") };
    assert_eq!(image.size, [1224, 792]);
    assert!(image.pixels.iter().all(|c| *c == egui::Color32::WHITE), "a blank page renders white and opaque");
    let (scale, size) = greenline::render::thumb_size(vec2(1224.0, 792.0));
    let Resp::Rendered { layer: Layer::Thumb, image, scale: s, .. } = next(&w) else { panic!("thumbnail") };
    assert_eq!((image.size, s), (size, scale));

    // Asked for again (the sidebar dropped it, say): answered from the one made.
    let t = Instant::now();
    w.send(Req::Render { generation: 1, view: 7, page: 0, scale, origin: [0, 0], size, layer: Layer::Thumb });
    let Resp::Rendered { layer: Layer::Thumb, image, .. } = next(&w) else { panic!("thumbnail again") };
    assert_eq!(image.size, size);
    assert!(t.elapsed() < Duration::from_secs(1));

    // Tiles come in the order asked for; tiles no longer wanted are dropped.
    w.keep.lock().unwrap().tiles = [(7, 1, [0, 0]), (7, 1, [512, 0])].into_iter().collect();
    for origin in [[512, 0], [0, 0], [1024, 0]] {
        w.send(Req::Render { generation: 1, view: 7, page: 1, scale: 2.0, origin, size: [512, 512], layer: Layer::Patch });
    }
    let mut got = Vec::new();
    while let Ok(r) = w.rx.recv_timeout(Duration::from_millis(1500)) {
        if let Resp::Rendered { layer: Layer::Patch, origin, image, .. } = r {
            assert_eq!(image.size, [512, 512]);
            got.push(origin);
        }
    }
    assert_eq!(got, [[512, 0], [0, 0]]);

    // Arranged anew (a blank page first, the last page deleted): a new generation.
    let l = greenline::pages::Layout::identity(3);
    let l = greenline::pages::insert_pages(&greenline::pages::delete_pages(&l, &[2]), 0, vec![greenline::pages::Src::Blank { width: 100.0, height: 50.0 }]);
    w.send(Req::Relayout { from: 1, generation: 2, path: pdf.clone(), layout: Some(l) });
    let Resp::Relaid { generation: 2, pages } = next(&w) else { panic!("relaid") };
    assert_eq!(pages, [vec2(100.0, 50.0), vec2(1224.0, 792.0), vec2(1224.0, 792.0)]);
    w.keep.lock().unwrap().base.insert((7, 0));
    w.send(Req::Render { generation: 1, view: 7, page: 0, scale: 1.0, origin: [0, 0], size: [10, 10], layer: Layer::Base });
    w.send(Req::Render { generation: 2, view: 7, page: 0, scale: 1.0, origin: [0, 0], size: [100, 50], layer: Layer::Base });
    let Resp::Rendered { generation: 2, image, .. } = next(&w) else { panic!("the old generation's render is dropped") };
    assert_eq!(image.size, [100, 50]);

    w.send(Req::Close { generation: 2 });
    std::fs::remove_dir_all(&dir).ok();
}
