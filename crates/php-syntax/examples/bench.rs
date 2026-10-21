//! Parse ~1 MB of generated PHP and report the time (run with `--release`).

use std::time::Instant;

fn source() -> String {
    let mut s = String::from("<?php\nnamespace Bench;\nuse Some\\Thing;\n");
    let mut i = 0;
    while s.len() < 1 << 20 {
        s.push_str(&format!(
            r#"
/** Doc comment for class {i} */
final class Service{i} extends Base implements \Countable {{
    private const LIMIT = 10 * {i};
    public function __construct(private readonly array $items = [], protected ?Thing $thing = null) {{}}
    public function count(): int {{ return \count($this->items); }}
    public function run(int $n, string ...$names): array {{
        $out = [];
        foreach ($this->items as $key => $value) {{
            if ($value > self::LIMIT && !isset($out[$key])) {{
                $out[$key] = "item {{$key}}: $value " . strtoupper($names[0] ?? 'none');
            }} elseif ($value === null) {{
                continue;
            }} else {{
                $out[] = match (true) {{ $n < 0 => -$n, $n % 2 === 0 => $n / 2, default => $n * 3 + 1 }};
            }}
        }}
        $fn = fn($x) => $x * {i} + ($this->thing?->value() ?? 0);
        $text = <<<EOT
            Service {i} produced {{$n}} results
            EOT;
        return array_map($fn, array_filter($out, static function ($v) use ($n): bool {{ return $v !== $n; }}));
    }}
}}
"#
        ));
        i += 1;
    }
    s
}

fn main() {
    let src = source();
    let mut best = f64::MAX;
    for _ in 0..10 {
        let t = Instant::now();
        let p = php_syntax::parse(src.as_bytes(), "bench.php").expect("parses");
        let dt = t.elapsed().as_secs_f64();
        assert!(!p.stmts.is_empty());
        best = best.min(dt);
    }
    let t = Instant::now();
    let toks = php_syntax::tokenize(src.as_bytes(), Default::default()).unwrap();
    let tok_time = t.elapsed().as_secs_f64();
    println!(
        "{:.2} MB: parse+resolve {:.1} ms (best of 10), token_get_all {:.1} ms ({} tokens)",
        src.len() as f64 / 1e6,
        best * 1e3,
        tok_time * 1e3,
        toks.len()
    );
}
