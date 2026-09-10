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
    ]
);
