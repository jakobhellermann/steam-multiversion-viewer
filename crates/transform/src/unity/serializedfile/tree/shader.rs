use crate::structured::Node;

use super::super::shader::{self, PlatformPrograms, ProgramRef};
use rabex_env::rabex::objects::pptr::PathId;

pub(super) fn shader_nodes(path_id: PathId, groups: Vec<PlatformPrograms>) -> Vec<Node> {
    groups
        .into_iter()
        .map(|group| {
            let total: usize = group.passes.iter().map(|pass| pass.programs.len()).sum();
            let children = if group.passes.len() == 1 {
                program_leaves(path_id, group.platform, &group.passes[0].programs)
            } else {
                group
                    .passes
                    .iter()
                    .enumerate()
                    .map(|(index, pass)| Node {
                        id: format!("obj:{path_id}/passgroup:{}:{index}", group.platform),
                        label: pass.label.clone(),
                        kind: "shader-pass".to_string(),
                        badge: Some(pass.programs.len().to_string()),
                        default_collapsed: true,
                        children: program_leaves(path_id, group.platform, &pass.programs),
                        ..Default::default()
                    })
                    .collect()
            };
            Node {
                id: format!("obj:{path_id}/plat:{}", group.platform),
                label: shader::platform_name(group.platform).to_string(),
                kind: "shader-platform".to_string(),
                badge: Some(total.to_string()),
                default_collapsed: true,
                children,
                ..Default::default()
            }
        })
        .collect()
}

fn program_leaves(path_id: PathId, platform: u32, programs: &[ProgramRef]) -> Vec<Node> {
    programs
        .iter()
        .map(|program| {
            let label = if program.keywords.is_empty() {
                program.stage.to_string()
            } else {
                format!("{} · {}", program.stage, program.keywords.join(", "))
            };
            let mut node = Node::leaf(
                format!("obj:{path_id}/prog:{platform}:{}", program.blob_index),
                label,
                "shader-program",
            )
            .with_facet("stage", program.stage)
            .with_badge(program.type_name);
            node.has_content = true;
            node
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unity::serializedfile::shader::PassPrograms;

    fn program(blob_index: u32, stage: &'static str, keywords: &[&str]) -> ProgramRef {
        ProgramRef {
            blob_index,
            type_name: "GLCore32",
            stage,
            keywords: keywords.iter().map(|keyword| keyword.to_string()).collect(),
        }
    }

    #[test]
    fn shader_nodes_group_multi_pass_inline_single() {
        let groups = vec![
            PlatformPrograms {
                platform: 15,
                passes: vec![
                    PassPrograms {
                        label: "Pass 0".to_string(),
                        programs: vec![
                            program(2, "vertex", &[]),
                            program(3, "vertex", &["USE_MASK"]),
                        ],
                    },
                    PassPrograms {
                        label: "Pass 1".to_string(),
                        programs: vec![program(4, "vertex", &[])],
                    },
                ],
            },
            PlatformPrograms {
                platform: 18,
                passes: vec![PassPrograms {
                    label: "Pass 0".to_string(),
                    programs: vec![program(7, "fragment", &[])],
                }],
            },
        ];

        insta::assert_yaml_snapshot!(shader_nodes(42, groups));
    }
}
