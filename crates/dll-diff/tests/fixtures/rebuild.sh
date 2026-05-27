#!/usr/bin/env bash
# Regenerate from.dll / to.dll for every fixture case under this
# directory. Run locally whenever you add or edit a case; the produced
# DLLs are checked in so tests stay hermetic and don't need `dotnet`.
#
# Usage: ./rebuild.sh              # rebuild every case
#        ./rebuild.sh case_name    # rebuild one case
set -euo pipefail

cd "$(dirname "$0")"
HERE="$PWD"

CASES=("${@:-}")
if [[ -z "${CASES[*]}" ]]; then
	CASES=()
	for d in */; do
		[[ -f "$d/from.cs" && -f "$d/to.cs" ]] && CASES+=("${d%/}")
	done
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/fixture.csproj" <<'EOF'
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>netstandard2.0</TargetFramework>
    <AssemblyName>fixture</AssemblyName>
    <Deterministic>true</Deterministic>
    <DebugType>none</DebugType>
    <DebugSymbols>false</DebugSymbols>
    <GenerateDocumentationFile>false</GenerateDocumentationFile>
    <EnableDefaultCompileItems>false</EnableDefaultCompileItems>
    <AppendTargetFrameworkToOutputPath>false</AppendTargetFrameworkToOutputPath>
    <ProduceReferenceAssembly>false</ProduceReferenceAssembly>
  </PropertyGroup>
  <ItemGroup>
    <Compile Include="source.cs" />
  </ItemGroup>
</Project>
EOF

build_one() {
	local case="$1" side="$2"
	local src="$HERE/$case/$side.cs"
	[[ -f "$src" ]] || {
		echo "missing $src" >&2
		return 1
	}
	cp "$src" "$TMP/source.cs"
	rm -rf "$TMP/bin" "$TMP/obj"
	(cd "$TMP" && dotnet build -c Release -nologo -v quiet >/dev/null)
	cp "$TMP/bin/Release/fixture.dll" "$HERE/$case/$side.dll"
	echo "  $case/$side.dll"
}

for case in "${CASES[@]}"; do
	echo "$case"
	build_one "$case" from
	build_one "$case" to
done
