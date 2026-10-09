# Security Policy

fitment reads untrusted STEP files, and its catalog and API will accept
uploads. Panics, unbounded memory use or hangs on crafted input, and any
way to read or change catalog data without authorization, are treated as
security issues.

## Reporting

Please do not open a public issue for a vulnerability. Use GitHub's
private vulnerability reporting on this repository
(**Security → Report a vulnerability**), or email jchultarsky@gmail.com.

You will get an acknowledgement within a week. Fixes are released as a
patch version with a note in the changelog and a GitHub security advisory.

Parser bugs usually belong to [stepq](https://github.com/jchultarsky/stepq);
report them there the same way, or here if unsure.

## Supported versions

Only the latest published version receives fixes.
