//! An insertion-ordered string map, which is what a Python `dict` is.

use std::fmt;

/// Field name -> target, in the order the fields were first set.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Fields(Vec<(String, String)>);

impl Fields {
    pub fn new() -> Self {
        Fields(Vec::new())
    }

    /// Set a field, keeping its original position if it is already present -- exactly what `dict.__setitem__` and `dict.update` do.
    pub fn insert(&mut self, field: impl Into<String>, target: impl Into<String>) {
        let field = field.into();
        let target = target.into();
        match self.0.iter_mut().find(|(name, _)| *name == field) {
            Some(slot) => slot.1 = target,
            None => self.0.push((field, target)),
        }
    }

    /// `dict.update`: every entry of `other`, in `other`'s order.
    pub fn extend(&mut self, other: &Fields) {
        for (field, target) in other.iter() {
            self.insert(field.clone(), target.clone());
        }
    }

    pub fn get(&self, field: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, target)| target.as_str())
    }

    pub fn contains_key(&self, field: &str) -> bool {
        self.get(field).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.0.iter().map(|(field, target)| (field, target))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Fields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.0.iter().map(|(k, v)| (k, v)))
            .finish()
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Fields {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(entries: I) -> Self {
        let mut fields = Fields::new();
        for (field, target) in entries {
            fields.insert(field, target);
        }
        fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_order_is_the_iteration_order() {
        let fields: Fields = [("b", "1"), ("a", "2"), ("c", "3")].into_iter().collect();
        let names: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["b", "a", "c"],
            "a sorted map would reshuffle the file"
        );
    }

    #[test]
    fn resetting_a_field_keeps_its_original_position() {
        let mut fields: Fields = [("a", "1"), ("b", "2")].into_iter().collect();
        fields.insert("a", "9");
        let entries: Vec<(&str, &str)> = fields
            .iter()
            .map(|(name, target)| (name.as_str(), target.as_str()))
            .collect();
        assert_eq!(entries, [("a", "9"), ("b", "2")]);
    }

    #[test]
    fn extend_appends_new_fields_and_overwrites_shared_ones_in_place() {
        let mut fields: Fields = [("a", "1"), ("b", "2")].into_iter().collect();
        let other: Fields = [("b", "9"), ("c", "3")].into_iter().collect();
        fields.extend(&other);
        let entries: Vec<(&str, &str)> = fields
            .iter()
            .map(|(name, target)| (name.as_str(), target.as_str()))
            .collect();
        assert_eq!(entries, [("a", "1"), ("b", "9"), ("c", "3")]);
    }

    #[test]
    fn an_absent_field_reads_as_absent() {
        let fields = Fields::new();
        assert_eq!(fields.get("a"), None);
        assert!(!fields.contains_key("a"));
        assert!(fields.is_empty());
    }
}
