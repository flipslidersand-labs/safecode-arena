function add(a: number, b: number): number {
  return a + b;
}

function sumValues(values: number[]): number {
  return values.reduce((acc, v) => acc + v, 0);
}

function fibonacci(n: number): number {
  if (n < 0) throw new Error("n must be non-negative");
  if (n === 0) return 0;
  let [a, b] = [0, 1];
  for (let i = 1; i < n; i++) {
    [a, b] = [b, a + b];
  }
  return b;
}
