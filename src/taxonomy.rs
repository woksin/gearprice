//! Reverb's category tree, and turning a slug a person typed into a filter that works.
//!
//! Two things make this worth a module rather than a string passed straight through.
//!
//! Reverb splits the tree across two parameters: `product_type` takes a root slug such as
//! `electric-guitars`, and `category` takes a leaf such as `solid-body` — but only
//! alongside its root. A leaf sent on its own is ignored.
//!
//! And an unrecognised slug in either parameter is ignored rather than rejected. Asking
//! for `electric-guitar` (singular) returns a 200 covering all 641,515 listings on the
//! site, which would be reported as the price class of electric guitars. So slugs are
//! resolved against the live tree first, and an unknown one is an error with suggestions.

use anyhow::{Result, bail};

use crate::reverb::{Category, Client};

/// A resolved position in the tree, ready to be applied to a search.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selector {
    pub product_type: String,
    pub category: Option<String>,
    /// What to call this in a report — "Electric Guitars / Solid Body".
    pub label: String,
}

pub struct Taxonomy {
    roots: Vec<Category>,
}

impl Taxonomy {
    pub fn load(client: &Client) -> Result<Self> {
        Ok(Self {
            roots: client.categories()?.categories,
        })
    }

    #[cfg(test)]
    pub fn from_roots(roots: Vec<Category>) -> Self {
        Self { roots }
    }

    pub fn roots(&self) -> &[Category] {
        &self.roots
    }

    /// Resolves a slug to the parameters that actually filter on it.
    ///
    /// Matches a root first, then any leaf. A leaf slug that appears under more than one
    /// root — `parts` and `accessories` share several — resolves to the first, and can be
    /// disambiguated by passing `root/leaf`.
    pub fn resolve(&self, slug: &str) -> Result<Selector> {
        let slug = slug.trim().trim_matches('/');
        if let Some((root, leaf)) = slug.split_once('/') {
            let root = self.root(root).ok_or_else(|| self.unknown(root))?;
            let leaf = root
                .subcategories
                .iter()
                .find(|candidate| candidate.slug == leaf)
                .ok_or_else(|| {
                    let known: Vec<&str> = root
                        .subcategories
                        .iter()
                        .map(|child| child.slug.as_str())
                        .collect();
                    anyhow::anyhow!(
                        "{root_name} has no subcategory {leaf:?}. It has: {}",
                        known.join(", "),
                        root_name = root.slug
                    )
                })?;
            return Ok(Selector {
                product_type: root.slug.clone(),
                category: Some(leaf.slug.clone()),
                label: display_name(leaf, root),
            });
        }

        if let Some(root) = self.root(slug) {
            return Ok(Selector {
                product_type: root.slug.clone(),
                category: None,
                label: display_name(root, root),
            });
        }
        for root in &self.roots {
            if let Some(leaf) = root
                .subcategories
                .iter()
                .find(|candidate| candidate.slug == slug)
            {
                return Ok(Selector {
                    product_type: root.slug.clone(),
                    category: Some(leaf.slug.clone()),
                    label: display_name(leaf, root),
                });
            }
        }
        Err(self.unknown(slug))
    }

    fn root(&self, slug: &str) -> Option<&Category> {
        self.roots.iter().find(|root| root.slug == slug)
    }

    fn unknown(&self, slug: &str) -> anyhow::Error {
        let suggestions = self.nearest(slug);
        let known: Vec<&str> = self.roots.iter().map(|root| root.slug.as_str()).collect();
        if suggestions.is_empty() {
            anyhow::anyhow!(
                "unknown category {slug:?}. Top-level categories are: {}. \
                 Run `gearprice categories` for the full tree.",
                known.join(", ")
            )
        } else {
            anyhow::anyhow!(
                "unknown category {slug:?}. Did you mean {}? \
                 Run `gearprice categories` for the full tree.",
                suggestions.join(" or ")
            )
        }
    }

    /// Slugs close enough to a mistyped one to be worth suggesting.
    fn nearest(&self, slug: &str) -> Vec<String> {
        let mut scored: Vec<(usize, String)> = Vec::new();
        for root in &self.roots {
            for candidate in std::iter::once(root).chain(root.subcategories.iter()) {
                let distance = edit_distance(slug, &candidate.slug);
                if distance <= 3 || candidate.slug.contains(slug) || slug.contains(&candidate.slug)
                {
                    scored.push((distance, candidate.slug.clone()));
                }
            }
        }
        scored.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        scored.dedup_by(|left, right| left.1 == right.1);
        scored
            .into_iter()
            .take(2)
            .map(|(_, slug)| format!("{slug:?}"))
            .collect()
    }
}

fn display_name(category: &Category, root: &Category) -> String {
    if !category.full_name.is_empty() {
        return category.full_name.clone();
    }
    if category.slug == root.slug {
        return category.name.clone();
    }
    format!("{} / {}", root.name, category.name)
}

/// Levenshtein distance, for "did you mean". Small inputs, so the simple matrix is fine.
fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (row, left_char) in left.iter().enumerate() {
        current[0] = row + 1;
        for (column, right_char) in right.iter().enumerate() {
            let substitution = usize::from(left_char != right_char);
            current[column + 1] = (previous[column] + substitution)
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// Fails a search that would silently cover the whole marketplace.
pub fn require_targeted(search: &crate::query::Search) -> Result<()> {
    if !search.is_targeted() {
        bail!("that would search the entire marketplace — give a query, a model or a category");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn category(slug: &str, name: &str, full_name: &str, children: Vec<Category>) -> Category {
        Category {
            slug: slug.into(),
            name: name.into(),
            full_name: full_name.into(),
            subcategories: children,
        }
    }

    fn tree() -> Taxonomy {
        Taxonomy::from_roots(vec![
            category(
                "electric-guitars",
                "Electric Guitars",
                "Electric Guitars",
                vec![
                    category(
                        "solid-body",
                        "Solid Body",
                        "Electric Guitars / Solid Body",
                        vec![],
                    ),
                    category(
                        "semi-hollow",
                        "Semi-Hollow",
                        "Electric Guitars / Semi-Hollow",
                        vec![],
                    ),
                ],
            ),
            category(
                "effects-and-pedals",
                "Effects and Pedals",
                "Effects and Pedals",
                vec![category(
                    "overdrive-and-boost",
                    "Overdrive and Boost",
                    "Effects and Pedals / Overdrive and Boost",
                    vec![],
                )],
            ),
        ])
    }

    #[test]
    fn a_root_slug_becomes_a_product_type() {
        let selector = tree().resolve("electric-guitars").unwrap();
        assert_eq!("electric-guitars", selector.product_type);
        assert_eq!(None, selector.category);
        assert_eq!("Electric Guitars", selector.label);
    }

    #[test]
    fn a_leaf_slug_carries_its_root_along_because_alone_it_is_ignored() {
        let selector = tree().resolve("solid-body").unwrap();
        assert_eq!("electric-guitars", selector.product_type);
        assert_eq!(Some("solid-body".to_string()), selector.category);
        assert_eq!("Electric Guitars / Solid Body", selector.label);
    }

    #[test]
    fn a_qualified_path_selects_the_leaf_under_the_named_root() {
        let selector = tree()
            .resolve("effects-and-pedals/overdrive-and-boost")
            .unwrap();
        assert_eq!("effects-and-pedals", selector.product_type);
        assert_eq!(Some("overdrive-and-boost".to_string()), selector.category);
    }

    #[test]
    fn an_unknown_slug_is_an_error_and_not_the_whole_marketplace() {
        // The failure that matters: a near miss must not quietly price all 641,515
        // listings on Reverb as if they were electric guitars.
        let error = tree().resolve("electric-guitar").unwrap_err().to_string();
        assert!(error.contains("unknown category"), "{error}");
        assert!(error.contains("electric-guitars"), "{error}");
    }

    #[test]
    fn a_wholly_unfamiliar_slug_lists_the_top_level_categories() {
        let error = tree().resolve("zzzzzzzzzz").unwrap_err().to_string();
        assert!(error.contains("Top-level categories are"), "{error}");
        assert!(error.contains("effects-and-pedals"), "{error}");
    }

    #[test]
    fn a_leaf_under_the_wrong_root_names_the_leaves_that_exist() {
        let error = tree()
            .resolve("effects-and-pedals/solid-body")
            .unwrap_err()
            .to_string();
        assert!(error.contains("overdrive-and-boost"), "{error}");
    }

    #[test]
    fn edit_distance_counts_the_edits() {
        assert_eq!(0, edit_distance("amps", "amps"));
        assert_eq!(1, edit_distance("amps", "amp"));
        assert_eq!(
            2,
            edit_distance("kitten", "sitten"[..].trim_end_matches('n'))
        );
        assert_eq!(3, edit_distance("kitten", "sitting"));
    }

    #[test]
    fn an_untargeted_search_is_refused() {
        use crate::query::Search;
        assert!(require_targeted(&Search::default()).is_err());
        assert!(require_targeted(&Search::text("gibson")).is_ok());
    }
}
