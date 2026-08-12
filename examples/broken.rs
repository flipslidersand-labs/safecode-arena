/// コンパイルエラーのサンプル候補。
/// safecode evaluate examples/broken.rs で compile ❌ を確認できる。

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

// 型不一致: &str を返す関数が i32 を返そうとしている
pub fn broken_return() -> &str {
    42
}

// 未定義変数の参照
pub fn use_undefined() -> i32 {
    undefined_variable + 1
}
