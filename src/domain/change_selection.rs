use std::{collections::HashSet, path::PathBuf};

use super::{ChangeRepresentation, ChangeSelection};

/// Selección múltiple de filas de la vista Cambios.
///
/// Todas las operaciones reciben `rows`: la lista de filas seleccionables tal
/// y como se ven en ese momento, en su orden visual. El estado nunca guarda
/// índices, de modo que una fila que desaparece por un cambio externo se
/// elimina de la selección en lugar de arrastrarla hacia la fila que ocupe su
/// posición.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangeSelectionState {
    selected: HashSet<ChangeSelection>,
    /// Extremo fijo de los rangos creados con Mayús.
    anchor: Option<ChangeSelection>,
    /// Fila activa para el teclado y extremo móvil de los rangos.
    lead: Option<ChangeSelection>,
}

impl ChangeSelectionState {
    /// Indica si una fila concreta está seleccionada.
    #[must_use]
    pub fn contains(&self, selection: &ChangeSelection) -> bool {
        self.selected.contains(selection)
    }

    /// Número de filas seleccionadas.
    #[must_use]
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    /// Indica si no hay ninguna fila seleccionada.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// Fila activa para la navegación por teclado.
    #[must_use]
    pub const fn lead(&self) -> Option<&ChangeSelection> {
        self.lead.as_ref()
    }

    /// Vacía la selección y sus extremos.
    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.lead = None;
    }

    /// Deja seleccionada únicamente la fila indicada (clic simple).
    pub fn select_only(&mut self, selection: ChangeSelection) {
        self.selected.clear();
        self.selected.insert(selection.clone());
        self.anchor = Some(selection.clone());
        self.lead = Some(selection);
    }

    /// Añade o quita una fila sin tocar el resto (Ctrl+clic).
    pub fn toggle(&mut self, selection: ChangeSelection) {
        if !self.selected.insert(selection.clone()) {
            self.selected.remove(&selection);
        }
        self.anchor = Some(selection.clone());
        self.lead = Some(selection);
    }

    /// Sustituye la selección por el rango entre el ancla y `target` (Mayús+clic).
    ///
    /// Si el ancla ya no está visible, `target` pasa a ser la nueva ancla.
    pub fn extend_to(&mut self, rows: &[ChangeSelection], target: &ChangeSelection) {
        let Some(target_index) = index_of(rows, target) else {
            return;
        };
        let anchor_index = self
            .anchor
            .as_ref()
            .and_then(|anchor| index_of(rows, anchor))
            .or_else(|| self.lead.as_ref().and_then(|lead| index_of(rows, lead)));
        let Some(anchor_index) = anchor_index else {
            self.select_only(target.clone());
            return;
        };
        let (start, end) = if anchor_index <= target_index {
            (anchor_index, target_index)
        } else {
            (target_index, anchor_index)
        };
        self.selected = rows[start..=end].iter().cloned().collect();
        self.anchor = Some(rows[anchor_index].clone());
        self.lead = Some(target.clone());
    }

    /// Añade el rango entre el ancla y `target` a lo ya seleccionado
    /// (Ctrl+Mayús+clic), sin descartar el resto.
    pub fn add_range_to(&mut self, rows: &[ChangeSelection], target: &ChangeSelection) {
        let previous = std::mem::take(&mut self.selected);
        self.extend_to(rows, target);
        self.selected.extend(previous);
    }

    /// Selecciona todas las filas seleccionables visibles.
    pub fn select_all(&mut self, rows: &[ChangeSelection]) {
        self.selected = rows.iter().cloned().collect();
        self.anchor = rows.first().cloned();
        self.lead = rows.last().cloned();
    }

    /// Mueve la fila activa y, con `extend`, arrastra el rango desde el ancla.
    ///
    /// Equivalente de teclado de Mayús+clic y del clic simple.
    pub fn move_lead(&mut self, rows: &[ChangeSelection], forward: bool, extend: bool) {
        if rows.is_empty() {
            self.clear();
            return;
        }
        let current = self.reference_index(rows);
        let next_index = match current {
            Some(index) if forward => index.saturating_add(1).min(rows.len() - 1),
            Some(index) => index.saturating_sub(1),
            None if forward => 0,
            None => rows.len() - 1,
        };
        let target = rows[next_index].clone();
        if extend {
            self.extend_to(rows, &target);
        } else {
            self.select_only(target);
        }
    }

    /// Descarta las filas que ya no existen tras un refresco, un filtro o el
    /// plegado de un grupo.
    pub fn retain_rows(&mut self, rows: &[ChangeSelection]) {
        let visible: HashSet<&ChangeSelection> = rows.iter().collect();
        self.selected
            .retain(|selection| visible.contains(selection));
        if self
            .anchor
            .as_ref()
            .is_some_and(|anchor| !visible.contains(anchor))
        {
            self.anchor = None;
        }
        if self
            .lead
            .as_ref()
            .is_some_and(|lead| !visible.contains(lead))
        {
            self.lead = None;
        }
    }

    /// Punto de partida del teclado.
    ///
    /// Si un cambio externo se llevó por delante la fila activa y su ancla, se
    /// retoma desde la primera fila que siga seleccionada en lugar de saltar a
    /// un extremo de la lista y perder el resto de la selección.
    fn reference_index(&self, rows: &[ChangeSelection]) -> Option<usize> {
        self.lead
            .as_ref()
            .and_then(|lead| index_of(rows, lead))
            .or_else(|| {
                self.anchor
                    .as_ref()
                    .and_then(|anchor| index_of(rows, anchor))
            })
            .or_else(|| rows.iter().position(|row| self.selected.contains(row)))
    }

    /// Filas seleccionadas en el orden en el que se ven.
    #[must_use]
    pub fn ordered(&self, rows: &[ChangeSelection]) -> Vec<ChangeSelection> {
        rows.iter()
            .filter(|selection| self.selected.contains(selection))
            .cloned()
            .collect()
    }

    /// Número de rutas distintas que afectaría una acción sobre estas
    /// representaciones, sin materializarlas.
    #[must_use]
    pub fn count_paths_for(
        &self,
        rows: &[ChangeSelection],
        representations: &[ChangeRepresentation],
    ) -> usize {
        let mut seen = HashSet::new();
        rows.iter()
            .filter(|selection| {
                self.selected.contains(selection)
                    && representations.contains(&selection.representation)
            })
            .filter(|selection| seen.insert(selection.path.as_path()))
            .count()
    }

    /// Rutas seleccionadas de una representación concreta, en orden y sin
    /// repetir.
    #[must_use]
    pub fn paths_for(
        &self,
        rows: &[ChangeSelection],
        representations: &[ChangeRepresentation],
    ) -> Vec<PathBuf> {
        let mut seen = HashSet::new();
        rows.iter()
            .filter(|selection| {
                self.selected.contains(selection)
                    && representations.contains(&selection.representation)
            })
            .filter(|selection| seen.insert(selection.path.clone()))
            .map(|selection| selection.path.clone())
            .collect()
    }
}

fn index_of(rows: &[ChangeSelection], selection: &ChangeSelection) -> Option<usize> {
    rows.iter().position(|row| row == selection)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{ChangeRepresentation, ChangeSelection, ChangeSelectionState};

    fn row(path: &str, representation: ChangeRepresentation) -> ChangeSelection {
        ChangeSelection::new(PathBuf::from(path), representation)
    }

    fn rows() -> Vec<ChangeSelection> {
        vec![
            row("a.txt", ChangeRepresentation::Staged),
            row("b.txt", ChangeRepresentation::Staged),
            row("a.txt", ChangeRepresentation::Worktree),
            row("c.txt", ChangeRepresentation::Untracked),
        ]
    }

    #[test]
    fn staged_and_worktree_rows_of_the_same_path_do_not_mix() {
        let rows = rows();
        let mut state = ChangeSelectionState::default();

        state.toggle(row("a.txt", ChangeRepresentation::Staged));

        assert!(state.contains(&row("a.txt", ChangeRepresentation::Staged)));
        assert!(!state.contains(&row("a.txt", ChangeRepresentation::Worktree)));
        assert_eq!(
            state.paths_for(&rows, &[ChangeRepresentation::Staged]),
            vec![PathBuf::from("a.txt")]
        );
        assert!(
            state
                .paths_for(
                    &rows,
                    &[
                        ChangeRepresentation::Worktree,
                        ChangeRepresentation::Untracked
                    ]
                )
                .is_empty()
        );
    }

    #[test]
    fn shift_selection_covers_the_visual_range_between_anchor_and_target() {
        let rows = rows();
        let mut state = ChangeSelectionState::default();

        state.select_only(row("c.txt", ChangeRepresentation::Untracked));
        state.extend_to(&rows, &row("b.txt", ChangeRepresentation::Staged));

        assert_eq!(state.len(), 3);
        assert_eq!(
            state.ordered(&rows),
            vec![
                row("b.txt", ChangeRepresentation::Staged),
                row("a.txt", ChangeRepresentation::Worktree),
                row("c.txt", ChangeRepresentation::Untracked),
            ]
        );
        // El ancla sigue siendo la fila del clic inicial, no la del rango.
        state.extend_to(&rows, &row("a.txt", ChangeRepresentation::Worktree));
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn external_changes_drop_missing_rows_without_shifting_by_position() {
        let mut state = ChangeSelectionState::default();
        state.select_all(&rows());

        // Git deja de reportar b.txt: el resto de filas conserva su identidad.
        let refreshed = vec![
            row("a.txt", ChangeRepresentation::Staged),
            row("a.txt", ChangeRepresentation::Worktree),
            row("c.txt", ChangeRepresentation::Untracked),
        ];
        state.retain_rows(&refreshed);

        assert_eq!(state.ordered(&refreshed), refreshed);
        assert!(!state.contains(&row("b.txt", ChangeRepresentation::Staged)));
    }

    #[test]
    fn losing_the_anchor_restarts_the_range_on_the_clicked_row() {
        let mut state = ChangeSelectionState::default();
        state.select_only(row("b.txt", ChangeRepresentation::Staged));

        let refreshed = vec![
            row("a.txt", ChangeRepresentation::Staged),
            row("a.txt", ChangeRepresentation::Worktree),
        ];
        state.retain_rows(&refreshed);
        state.extend_to(&refreshed, &row("a.txt", ChangeRepresentation::Worktree));

        assert_eq!(
            state.ordered(&refreshed),
            vec![row("a.txt", ChangeRepresentation::Worktree)]
        );
    }

    #[test]
    fn keyboard_navigation_extends_from_the_anchor_and_stops_at_the_edges() {
        let rows = rows();
        let mut state = ChangeSelectionState::default();

        state.move_lead(&rows, true, false);
        assert_eq!(state.ordered(&rows), vec![rows[0].clone()]);

        state.move_lead(&rows, true, true);
        state.move_lead(&rows, true, true);
        assert_eq!(state.ordered(&rows), rows[0..3].to_vec());

        state.move_lead(&rows, false, true);
        assert_eq!(state.ordered(&rows), rows[0..2].to_vec());

        state.move_lead(&rows, false, false);
        state.move_lead(&rows, false, false);
        assert_eq!(state.ordered(&rows), vec![rows[0].clone()]);
    }

    #[test]
    fn arrow_keys_do_not_jump_to_the_list_edge_after_losing_the_active_row() {
        let mut state = ChangeSelectionState::default();
        state.select_only(row("c.txt", ChangeRepresentation::Untracked));
        state.toggle(row("a.txt", ChangeRepresentation::Worktree));

        // Desaparece justo la fila activa (y ancla): a.txt worktree.
        let refreshed = vec![
            row("a.txt", ChangeRepresentation::Staged),
            row("b.txt", ChangeRepresentation::Staged),
            row("c.txt", ChangeRepresentation::Untracked),
        ];
        state.retain_rows(&refreshed);

        state.move_lead(&refreshed, true, false);

        // Reanuda junto a c.txt, que sigue seleccionada, no en la fila 0.
        assert_eq!(
            state.lead(),
            Some(&row("c.txt", ChangeRepresentation::Untracked))
        );
    }

    #[test]
    fn ctrl_shift_adds_the_range_without_discarding_the_rest() {
        let rows = rows();
        let mut state = ChangeSelectionState::default();
        state.toggle(rows[0].clone());
        state.toggle(rows[1].clone());

        // Ctrl+Mayús+clic sobre la última fila: el ancla es rows[1].
        state.add_range_to(&rows, &rows[3]);

        assert_eq!(
            state.ordered(&rows),
            rows,
            "el rango se suma a lo ya seleccionado en lugar de sustituirlo"
        );
    }

    #[test]
    fn navigation_over_an_empty_list_clears_the_selection() {
        let mut state = ChangeSelectionState::default();
        state.select_all(&rows());

        state.move_lead(&[], true, false);

        assert!(state.is_empty());
        assert!(state.lead().is_none());
    }
}
