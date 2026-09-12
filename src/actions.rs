use gpui::actions;

actions!(
    git_helper,
    [
        OpenRepository,
        CloseActiveRepository,
        NextRepository,
        PreviousRepository,
        RefreshRepository,
        ShowHistory,
        ShowChanges,
        CreateCommit,
        GenerateCommitMessage,
        StageSelection,
        UnstageSelection,
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
