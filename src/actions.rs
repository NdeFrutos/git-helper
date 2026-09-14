use gpui::actions;

actions!(
    git_helper,
    [
        OpenRepository,
        CloneRepository,
        CloseActiveRepository,
        NextRepository,
        PreviousRepository,
        RefreshRepository,
        ShowHistory,
        ShowChanges,
        ShowSummary,
        SummaryNextRow,
        SummaryPreviousRow,
        SummaryActivateRow,
        CreateCommit,
        GenerateCommitMessage,
        StageSelection,
        UnstageSelection,
        FindInView,
    ]
);

actions!(
    git_helper_change_list,
    [
        FocusNextChange,
        FocusPreviousChange,
        ExtendSelectionToNextChange,
        ExtendSelectionToPreviousChange,
        ToggleFocusedChange,
        SelectAllChanges,
        ClearChangeSelection,
    ]
);
