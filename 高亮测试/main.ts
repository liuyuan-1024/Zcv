interface User {
  readonly name: string;
  roles: string[];
}

type Result<T> = { ok: true; value: T } | { ok: false; error: Error };

function loadUser(id: number): Result<User> {
  if (id <= 0) return { ok: false, error: new Error("invalid id") };
  return { ok: true, value: { name: "Zcv", roles: ["editor"] } };
}

const result = loadUser(1);
if (result.ok) {
  console.log(result.value.name, result.value.roles.length);
}
