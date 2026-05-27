#!/usr/bin/env bash
# Build a single fixture .dll. Used by the Makefile — each invocation
# is a clean dotnet build of a throwaway csproj that pulls in the
# requested .cs sources (and optionally references one extra .dll).
#
# Usage: build-dll.sh OUT.dll SRC.cs [SRC.cs ...] [--ref LIB.dll]
set -euo pipefail

out=""
refs=()
srcs=()
while (($#)); do
	case "$1" in
	--ref)
		refs+=("$2")
		shift 2
		;;
	*)
		if [[ -z "$out" ]]; then
			out="$1"
		else
			srcs+=("$1")
		fi
		shift
		;;
	esac
done

[[ -n "$out" && ${#srcs[@]} -gt 0 ]] || {
	echo "usage: $0 OUT.dll SRC.cs [SRC.cs ...] [--ref LIB.dll]" >&2
	exit 1
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for s in "${srcs[@]}"; do cp "$s" "$tmp/"; done

# Compile + reference items. References use absolute HintPaths so the
# project file doesn't care about its own working directory.
compile_items=""
for s in "${srcs[@]}"; do
	compile_items+="    <Compile Include=\"$(basename "$s")\" />"$'\n'
done
ref_items=""
for r in "${refs[@]}"; do
	ref_items+="    <Reference Include=\"$(basename "$r" .dll)\"><HintPath>$(cd "$(dirname "$r")" && pwd)/$(basename "$r")</HintPath></Reference>"$'\n'
done

cat >"$tmp/p.csproj" <<PROJ
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>netstandard2.0</TargetFramework>
    <AssemblyName>p</AssemblyName>
    <Deterministic>true</Deterministic>
    <DebugType>none</DebugType>
    <DebugSymbols>false</DebugSymbols>
    <GenerateDocumentationFile>false</GenerateDocumentationFile>
    <EnableDefaultCompileItems>false</EnableDefaultCompileItems>
    <AppendTargetFrameworkToOutputPath>false</AppendTargetFrameworkToOutputPath>
    <ProduceReferenceAssembly>false</ProduceReferenceAssembly>
  </PropertyGroup>
  <ItemGroup>
${compile_items}${ref_items}
  </ItemGroup>
</Project>
PROJ

(cd "$tmp" && dotnet build -c Release -nologo -v quiet >/dev/null)
cp "$tmp/bin/Release/p.dll" "$out"
echo "  $out"
