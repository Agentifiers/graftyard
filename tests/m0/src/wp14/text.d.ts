// The controller imports the containment harness as a text module (wrangler `rules`, type Text).
declare module "*.py" {
  const text: string;
  export default text;
}
