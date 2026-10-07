// 检查用例直接接入 cargo test，避免 docs 下的回归验证被遗漏。
#[path = "../docs/reviews/system-review-probes.rs"]
mod architecture_review;

#[path = "../docs/reviews/system-extension-probes.rs"]
mod extension_review;
