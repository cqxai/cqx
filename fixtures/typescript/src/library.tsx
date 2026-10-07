export function render(input: string) {
  // @ts-ignore
  eval(input);
  try { optional(); } catch {}
  return <p>{input}</p>;
}
